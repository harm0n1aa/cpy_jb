// Walk SpringBoard's UIKit tree, then (if available) the frontmost app's
// accessibility tree. The dylib loads only in SpringBoard so Dopamine's
// userspace reboot is not blocked by UIKit-wide injection.

#import "UiDump.h"

#import <UIKit/UIKit.h>
#import <objc/message.h>
#import <objc/runtime.h>

static NSString *viewText(UIView *v) {
    NSMutableArray *parts = [NSMutableArray array];
    void (^add)(NSString *) = ^(NSString *t) {
        if (t.length) {
            [parts addObject:t];
        }
    };
    add(v.accessibilityLabel);
    add(v.accessibilityValue);
    add(v.accessibilityIdentifier);
    if ([v isKindOfClass:[UILabel class]]) {
        add(((UILabel *)v).text);
    }
    if ([v isKindOfClass:[UIButton class]]) {
        add([(UIButton *)v currentTitle]);
        add([(UIButton *)v accessibilityLabel]);
    }
    if ([v isKindOfClass:[UITextField class]]) {
        UITextField *tf = (UITextField *)v;
        add(tf.text);
        add(tf.placeholder);
    }
    if ([v respondsToSelector:@selector(text)]) {
        id t = ((id (*)(id, SEL))objc_msgSend)(v, @selector(text));
        if ([t isKindOfClass:[NSString class]]) {
            add(t);
        }
    }
    Class iconView = NSClassFromString(@"SBIconView");
    if (iconView && [v isKindOfClass:iconView] && [v respondsToSelector:@selector(icon)]) {
        id icon = ((id (*)(id, SEL))objc_msgSend)(v, @selector(icon));
        if ([icon respondsToSelector:@selector(displayName)]) {
            add(((id (*)(id, SEL))objc_msgSend)(icon, @selector(displayName)));
        }
        if ([icon respondsToSelector:@selector(applicationBundleID)]) {
            add(((id (*)(id, SEL))objc_msgSend)(icon, @selector(applicationBundleID)));
        }
    }
    if (parts.count == 0) {
        return @"";
    }
    return [[NSSet setWithArray:parts].allObjects componentsJoinedByString:@" "];
}

static void walk(UIView *v, NSMutableArray *nodes, CGFloat sw, CGFloat sh, int *budget) {
    if (*budget <= 0 || !v || v.hidden || v.alpha < 0.02) {
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

static NSDictionary *dumpLocal(void) {
    CGRect sb = [UIScreen mainScreen].bounds;
    CGFloat sw = MAX(sb.size.width, 1);
    CGFloat sh = MAX(sb.size.height, 1);
    NSMutableArray *nodes = [NSMutableArray array];
    int budget = 450;
    NSArray *windows = [UIApplication sharedApplication].windows;
    for (UIWindow *w in windows) {
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
