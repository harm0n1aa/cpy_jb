#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>
#import <objc/runtime.h>

#import "StreamClient.h"
#import "InputInjector.h"
#import "KeyboardSuppression.h"
#import "UiDump.h"

@interface SBUserNotificationAlert : NSObject
- (void)_setActivated:(BOOL)activated;
- (void)_sendResponseAndCleanUp:(BOOL)cleanup;
@end

static BOOL gSuppressPasteAlert = NO;
static void (*gOrigActivateAlertItem)(id, SEL, id) = NULL;

static void IOSPYActivateAlertItem(id self, SEL sel, id arg1) {
    if (gSuppressPasteAlert && arg1) {
        Class cls = NSClassFromString(@"SBUserNotificationAlert");
        if (cls && [arg1 isKindOfClass:cls]) {
            NSString *source = nil;
            Ivar iv = class_getInstanceVariable(object_getClass(arg1), "_alertSource");
            if (iv) {
                @try {
                    source = object_getIvar(arg1, iv);
                } @catch (__unused id e) {
                    source = nil;
                }
            }
            if ([source isKindOfClass:[NSString class]] && [source isEqualToString:@"pasted"] &&
                [arg1 respondsToSelector:@selector(_setActivated:)] &&
                [arg1 respondsToSelector:@selector(_sendResponseAndCleanUp:)]) {
                [arg1 _setActivated:NO];
                [arg1 _sendResponseAndCleanUp:YES];
                return;
            }
        }
    }
    if (gOrigActivateAlertItem) {
        gOrigActivateAlertItem(self, sel, arg1);
    }
}

static void IOSPYInstallPasteHook(void) {
    Class cls = NSClassFromString(@"SBAlertItem");
    if (!cls) {
        return;
    }
    Method m = class_getClassMethod(cls, sel_registerName("activateAlertItem:"));
    if (!m) {
        return;
    }
    gOrigActivateAlertItem = (void (*)(id, SEL, id))method_getImplementation(m);
    method_setImplementation(m, (IMP)IOSPYActivateAlertItem);
}

__attribute__((constructor)) static void IOSPYTweakInit(void) {
    @autoreleasepool {
        NSLog(@"[ioscpyhook] loaded (v0.1.24)");
        NSString *dir = @"/var/mobile/Library/Preferences";
        NSString *marker = [dir stringByAppendingPathComponent:@"com.ioscpy.hook.loaded"];
        [@"1" writeToFile:marker atomically:YES encoding:NSUTF8StringEncoding error:nil];
        NSOperatingSystemVersion v = [[NSProcessInfo processInfo] operatingSystemVersion];
        gSuppressPasteAlert = (v.majorVersion >= 16);
        // Constructor runs while SpringBoard is still coming up. Touching UIKit /
        // sockets here is a common reason ElleKit unloads the dylib with no crash.
        int64_t delayNs = [UIApplication sharedApplication] ? (int64_t)(0.2 * NSEC_PER_SEC)
                                                            : (int64_t)(2 * NSEC_PER_SEC);
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, delayNs),
                       dispatch_get_main_queue(), ^{
                           [[IOSPYStreamClient shared] start];
                           IOSPYInstallPasteHook();
                           IOSPYUiDumpStart();
                           IOSPYTextInjectionStart();
                           IOSPYOrientationStart();
                           IOSPYInputWarmup();
                           IOSPYKeyboardSuppressionInit();
                       });
    }
}
