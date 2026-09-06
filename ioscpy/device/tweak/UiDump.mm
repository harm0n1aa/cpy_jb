// Walk SpringBoard's UIKit tree (home + lock/passcode). The dylib loads only
// in SpringBoard so Dopamine's userspace reboot is not blocked by UIKit-wide
// injection.

#import "UiDump.h"

#import <UIKit/UIKit.h>
#import <objc/message.h>
#import <objc/runtime.h>

static void addStr(NSMutableArray *parts, NSString *t) {
    if (t.length) {
        [parts addObject:t];
    }
}

static BOOL classNameHas(UIView *v, NSString *needle) {
    NSString *n = NSStringFromClass(v.class);
    return n && [n rangeOfString:needle options:NSCaseInsensitiveSearch].location != NSNotFound;
}

static BOOL isPasscodeKey(UIView *v) {
    static NSArray<NSString *> *needles;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        needles = @[
            @"PasscodeNumberPad", @"NumberPadButton", @"TPNumberPad", @"DialerNumberPad",
            @"SBUIPasscode", @"SBPasscode", @"PinKey", @"PINButton", @"NumericButton"
        ];
    });
    NSString *n = NSStringFromClass(v.class) ?: @"";
    for (NSString *s in needles) {
        if ([n rangeOfString:s options:NSCaseInsensitiveSearch].location != NSNotFound) {
            return YES;
        }
    }
    return NO;
}

// Lock-screen keys often expose the digit via character/digit, not UILabel.text.
static NSString *passcodeDigit(UIView *v) {
    SEL sels[] = {
        @selector(character), @selector(digit), @selector(number), @selector(stringValue),
        @selector(title), @selector(string), @selector(keyCharacter), 0
    };
    for (int i = 0; sels[i]; i++) {
        if (![v respondsToSelector:sels[i]]) {
            continue;
        }
        id val = ((id (*)(id, SEL))objc_msgSend)(v, sels[i]);
        if ([val isKindOfClass:[NSString class]] && [val length]) {
            unichar c = [val characterAtIndex:0];
            if (c >= '0' && c <= '9') {
                return [NSString stringWithCharacters:&c length:1];
            }
            // Sometimes "1\nABC" style
            NSCharacterSet *digits = [NSCharacterSet decimalDigitCharacterSet];
            NSRange r = [val rangeOfCharacterFromSet:digits];
            if (r.location != NSNotFound) {
                return [val substringWithRange:r];
            }
        } else if ([val isKindOfClass:[NSNumber class]]) {
            int d = [val intValue];
            if (d >= 0 && d <= 9) {
                return [NSString stringWithFormat:@"%d", d];
            }
        }
    }
    // Walk one level of labels inside the key.
    for (UIView *c in v.subviews) {
        if ([c isKindOfClass:[UILabel class]]) {
            NSString *t = ((UILabel *)c).text;
            if (t.length) {
                unichar ch = [t characterAtIndex:0];
                if (ch >= '0' && ch <= '9') {
                    return [NSString stringWithCharacters:&ch length:1];
                }
            }
        }
    }
    return nil;
}

static NSString *viewText(UIView *v) {
    NSMutableArray *parts = [NSMutableArray array];
    addStr(parts, v.accessibilityLabel);
    addStr(parts, v.accessibilityValue);
    addStr(parts, v.accessibilityIdentifier);
    if ([v isKindOfClass:[UILabel class]]) {
        addStr(parts, ((UILabel *)v).text);
    }
    if ([v isKindOfClass:[UIButton class]]) {
        addStr(parts, [(UIButton *)v currentTitle]);
        addStr(parts, [(UIButton *)v accessibilityLabel]);
    }
    if ([v isKindOfClass:[UITextField class]]) {
        UITextField *tf = (UITextField *)v;
        addStr(parts, tf.text);
        addStr(parts, tf.placeholder);
    }
    if ([v respondsToSelector:@selector(text)]) {
        id t = ((id (*)(id, SEL))objc_msgSend)(v, @selector(text));
        if ([t isKindOfClass:[NSString class]]) {
            addStr(parts, t);
        }
    }
    NSString *digit = passcodeDigit(v);
    if (digit) {
        addStr(parts, digit);
    }
    Class iconView = NSClassFromString(@"SBIconView");
    if (iconView && [v isKindOfClass:iconView] && [v respondsToSelector:@selector(icon)]) {
        id icon = ((id (*)(id, SEL))objc_msgSend)(v, @selector(icon));
        if ([icon respondsToSelector:@selector(displayName)]) {
            addStr(parts, ((id (*)(id, SEL))objc_msgSend)(icon, @selector(displayName)));
        }
        if ([icon respondsToSelector:@selector(applicationBundleID)]) {
            addStr(parts, ((id (*)(id, SEL))objc_msgSend)(icon, @selector(applicationBundleID)));
        }
    }
    if (parts.count == 0) {
        return @"";
    }
    return [[NSSet setWithArray:parts].allObjects componentsJoinedByString:@" "];
}

static void walk(UIView *v, NSMutableArray *nodes, CGFloat sw, CGFloat sh, int *budget) {
    if (*budget <= 0 || !v || v.hidden) {
        return;
    }
    // Lock passcode keys can briefly sit at low alpha while animating in.
    if (v.alpha < 0.02 && !isPasscodeKey(v)) {
        return;
    }
    CGRect r = [v convertRect:v.bounds toView:nil];
    if (v.window) {
        r = [v.window convertRect:r toWindow:nil];
    }
    if (r.size.width >= 6 && r.size.height >= 6 &&
        r.origin.x < sw && r.origin.y < sh && r.origin.x + r.size.width > 0 &&
        r.origin.y + r.size.height > 0) {
        NSString *text = viewText(v);
        BOOL control = [v isKindOfClass:[UIControl class]] || [v isKindOfClass:[UIButton class]] ||
                       [v isKindOfClass:[UILabel class]] || [v isKindOfClass:[UIImageView class]];
        Class iconView = NSClassFromString(@"SBIconView");
        if (iconView && [v isKindOfClass:iconView]) {
            control = YES;
        }
        if (isPasscodeKey(v)) {
            control = YES;
            if (text.length == 0) {
                NSString *d = passcodeDigit(v);
                if (d) {
                    text = d;
                }
            }
        }
        if (text.length > 0 || (control && r.size.width >= 20 && r.size.height >= 16)) {
            NSString *bid = @"";
            if (iconView && [v isKindOfClass:iconView] && [v respondsToSelector:@selector(icon)]) {
                id icon = ((id (*)(id, SEL))objc_msgSend)(v, @selector(icon));
                if ([icon respondsToSelector:@selector(applicationBundleID)]) {
                    id b = ((id (*)(id, SEL))objc_msgSend)(icon, @selector(applicationBundleID));
                    if ([b isKindOfClass:[NSString class]]) {
                        bid = b;
                    }
                }
            }
            [nodes addObject:@{
                @"text" : text ?: @"",
                @"id" : v.accessibilityIdentifier ?: @"",
                @"class" : NSStringFromClass(v.class) ?: @"",
                @"bundle" : bid,
                @"x" : @(CGRectGetMidX(r) / sw),
                @"y" : @(CGRectGetMidY(r) / sh),
                @"w" : @(r.size.width / sw),
                @"h" : @(r.size.height / sh),
            }];
            (*budget)--;
        }
    }
    for (UIView *c in v.subviews) {
        walk(c, nodes, sw, sh, budget);
    }
}

static NSArray<UIWindow *> *allWindows(void) {
    NSMutableArray *out = [NSMutableArray array];
    NSMutableSet *seen = [NSMutableSet set];
    void (^add)(UIWindow *) = ^(UIWindow *w) {
        if (!w || [seen containsObject:w]) {
            return;
        }
        [seen addObject:w];
        [out addObject:w];
    };
    for (UIWindow *w in [UIApplication sharedApplication].windows) {
        add(w);
    }
    id app = [UIApplication sharedApplication];
    if ([app respondsToSelector:@selector(connectedScenes)]) {
        NSSet *scenes = ((NSSet * (*)(id, SEL))objc_msgSend)(app, @selector(connectedScenes));
        for (id scene in scenes) {
            if (![scene respondsToSelector:@selector(windows)]) {
                continue;
            }
            NSArray *ws = ((NSArray * (*)(id, SEL))objc_msgSend)(scene, @selector(windows));
            for (UIWindow *w in ws) {
                add(w);
            }
        }
    }
    // Private fallback — lock UI sometimes lives only here.
    if ([app respondsToSelector:NSSelectorFromString(@"_windows")]) {
        NSArray *ws = ((NSArray * (*)(id, SEL))objc_msgSend)(app, NSSelectorFromString(@"_windows"));
        for (UIWindow *w in ws) {
            add(w);
        }
    }
    return out;
}

static NSDictionary *dumpLocal(void) {
    CGRect sb = [UIScreen mainScreen].bounds;
    CGFloat sw = MAX(sb.size.width, 1);
    CGFloat sh = MAX(sb.size.height, 1);
    NSMutableArray *nodes = [NSMutableArray array];
    int budget = 600;
    for (UIWindow *w in allWindows()) {
        if (w.hidden || w.alpha < 0.02) {
            continue;
        }
        walk(w, nodes, sw, sh, &budget);
    }
    NSString *bid = [NSBundle mainBundle].bundleIdentifier ?: @"";
    return @{@"bundle" : bid, @"nodes" : nodes};
}

NSData *IOSPYPerformUiDump(void) {
    __block NSDictionary *dump = nil;
    void (^work)(void) = ^{
        dump = dumpLocal();
    };
    if ([NSThread isMainThread]) {
        work();
    } else {
        dispatch_sync(dispatch_get_main_queue(), work);
    }
    if (!dump) {
        dump = @{@"bundle" : @"", @"nodes" : @[]};
    }
    return [NSJSONSerialization dataWithJSONObject:dump options:0 error:nil] ?: [NSData data];
}

void IOSPYUiDumpStart(void) {
}
