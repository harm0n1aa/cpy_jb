#import "Diagnostics.h"
#import "Paths.h"
#import "Detect.h"
#import "FrameIngest.h"
#import "Inject.h"
#import "SpringBoardLookup.h"
#import "Capture.h"
#import "ControlServer.h"

#import <dlfcn.h>
#import <mach/mach.h>
#import <unistd.h>

static NSMutableArray<NSString *> *gLines = nil;

void IOSPYDiagReset(void) {
    gLines = [NSMutableArray array];
}

void IOSPYDiag(NSString *line) {
    if (!line.length) {
        return;
    }
    if (!gLines) {
        IOSPYDiagReset();
    }
    [gLines addObject:line];
    NSLog(@"[ioscpy/diag] %@", line);
}

void IOSPYDiagf(NSString *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    IOSPYDiag([[NSString alloc] initWithFormat:fmt arguments:ap]);
    va_end(ap);
}

NSArray<NSString *> *IOSPYDiagLines(void) {
    return gLines ?: @[];
}

NSString *IOSPYDiagSummary(void) {
    for (NSString *line in IOSPYDiagLines()) {
        if ([line containsString:@"FAIL"] || [line containsString:@"✗"]) {
            return line;
        }
    }
    NSString *last = IOSPYDiagLines().lastObject;
    return last.length ? last : @"";
}

static NSString *shortPath(NSString *path) {
    if (path.length <= 64) {
        return path ?: @"";
    }
    // Keep the useful tail (…/procursus/usr/lib/…) instead of the preboot UUID.
    NSRange r = [path rangeOfString:@"/procursus" options:NSBackwardsSearch];
    if (r.location != NSNotFound) {
        return [@"…" stringByAppendingString:[path substringFromIndex:r.location]];
    }
    return [@"…" stringByAppendingString:[path substringFromIndex:path.length - 48]];
}

void IOSPYDiagRunSession(void) {
    IOSPYDiagReset();
    IOSPYDiagf(@"ioscpy %@  jb=%@  uid=%d", IOSPYDaemonVersion,
               IOSPYLayoutName(IOSPYDetectLayout()), getuid());
    IOSPYDiagf(@"prefix %@", shortPath(IOSPYJBPrefix().length ? IOSPYJBPrefix() : @"/"));

    NSString *hookPath = IOSPYPath(@"/usr/lib/TweakInject/ioscpyhook.dylib");
    BOOL hookFile = [[NSFileManager defaultManager] fileExistsAtPath:hookPath];
    IOSPYDiagf(@"%@ hook file  %@", hookFile ? @"OK" : @"FAIL", shortPath(hookPath));

    BOOL hookMarker =
        [[NSFileManager defaultManager] fileExistsAtPath:
            @"/var/mobile/Library/Preferences/com.ioscpy.hook.loaded"];
    IOSPYDiagf(@"%@ hook ctor   %@", hookMarker ? @"OK" : @"FAIL",
               hookMarker ? @"ran in SpringBoard" : @"ElleKit did not load dylib");

    BOOL tweakSock = [[IOSPYFrameIngest shared] tweakConnected];
    uint64_t frames = [[IOSPYFrameIngest shared] framesFromTweak];
    NSUInteger lastBytes = [[IOSPYFrameIngest shared] lastFrameBytes];
    IOSPYDiagf(@"%@ tweak sock  %@  frames=%llu last=%zuB", tweakSock ? @"OK" : @"FAIL",
               tweakSock ? @"connected" : @"down", frames, (size_t)lastBytes);

    pid_t sbPid = IOSPYSpringBoardPid();
    IOSPYDiagf(@"%@ SpringBoard pid=%d", sbPid > 0 ? @"OK" : @"FAIL", sbPid);

    NSString *cap = IOSPYCaptureLastNote();
    IOSPYDiagf(@"capture note  %@", cap.length ? cap : @"(none yet)");

    if (tweakSock && frames == 0) {
        IOSPYDiag(@"WARN tweak connected but 0 video frames — capture failing in SpringBoard");
        IOSPYDiag(@"hint reconnect host after install; check note above for tweak-* backend");
    } else if (tweakSock && frames > 0) {
        IOSPYDiag(@"OK tweak is sending frames to daemon");
    } else if (!hookMarker) {
        IOSPYDiag(@"FAIL tweak not loaded — respring after Sileo install");
    } else if (!tweakSock) {
        IOSPYDiag(@"FAIL tweak loaded but socket down — restart ioscpyd / userspace reboot");
    }
}
