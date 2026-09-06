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

void IOSPYDiagRunSession(void) {
    IOSPYDiagReset();
    IOSPYDiagf(@"── ioscpy %@ ──", IOSPYDaemonVersion);

    NSString *prefix = IOSPYJBPrefix();
    IOSPYDiagf(@"jb: %@ prefix=%@ uid=%d", IOSPYLayoutName(IOSPYDetectLayout()),
               prefix.length ? prefix : @"/", getuid());

    NSString *hookPath = IOSPYPath(@"/usr/lib/TweakInject/ioscpyhook.dylib");
    BOOL hookFile = [[NSFileManager defaultManager] fileExistsAtPath:hookPath];
    IOSPYDiagf(@"%@ hook dylib: %@", hookFile ? @"OK" : @"FAIL", hookPath);

    BOOL hookMarker =
        [[NSFileManager defaultManager] fileExistsAtPath:
            @"/var/mobile/Library/Preferences/com.ioscpy.hook.loaded"];
    IOSPYDiagf(@"%@ hook marker (ctor ran): %@", hookMarker ? @"OK" : @"—",
               hookMarker ? @"yes" : @"no — ElleKit не загрузил dylib в SpringBoard");

    BOOL tweakSock = [[IOSPYFrameIngest shared] tweakConnected];
    IOSPYDiagf(@"%@ tweak→daemon socket: %@", tweakSock ? @"OK" : @"—",
               tweakSock ? @"connected" : @"not connected (кадр только из демона)");

    pid_t sbPid = IOSPYSpringBoardPid();
    IOSPYDiagf(@"%@ SpringBoard pid: %d", sbPid > 0 ? @"OK" : @"FAIL", sbPid);

    task_t sbTask = MACH_PORT_NULL;
    if (sbPid > 0 && IOSPYSpringBoardTask(&sbTask)) {
        IOSPYDiag(@"OK task_for_pid(SpringBoard)");
        uint32_t dyldVer = 0, dyldCount = 0;
        IOSPYDyldImageStats(sbTask, &dyldVer, &dyldCount);
        IOSPYDiagf(@"   dyld images: version=%u count=%u", dyldVer, dyldCount);
        IOSPYDiagf(@"   %s", IOSPYDyldSamplePaths(sbTask).UTF8String);
        mach_port_deallocate(mach_task_self(), sbTask);
    } else if (sbPid > 0) {
        IOSPYDiag(@"FAIL task_for_pid(SpringBoard) — нет прав (entitlements / jailbreak)");
    }

    NSString *inj = IOSPYInjectLastError();
    if (inj.length) {
        IOSPYDiagf(@"inject: %@", inj);
    } else {
        IOSPYDiag(@"inject: not attempted yet");
    }

    uint32_t renderClient = 0;
    if (IOSPYSpringBoardRenderClient(&renderClient)) {
        IOSPYDiagf(@"OK CARenderServer port from SpringBoard bootstrap: 0x%x", renderClient);
    } else {
        IOSPYDiag(@"FAIL CARenderServer port — bootstrap SpringBoard недоступен");
    }

    BOOL renderFn = IOSPYCaptureHasRenderFn();
    IOSPYDiagf(@"%@ CARenderServerRenderDisplay in демоне", renderFn ? @"OK" : @"FAIL");

    int fbW = 0, fbH = 0;
    IOSPYCaptureProbeFramebuffer(&fbW, &fbH);
    if (fbW > 0 && fbH > 0) {
        IOSPYDiagf(@"OK IOMobileFramebuffer: %dx%d", fbW, fbH);
    } else {
        IOSPYDiag(@"— IOMobileFramebuffer: нет размера (нормально для launchd)");
    }

    NSString *cap = IOSPYCaptureLastNote();
    IOSPYDiagf(@"capture last: %@", cap.length ? cap : @"(ещё не пробовали)");

    if (!hookMarker && !tweakSock) {
        IOSPYDiag(@"── почему нет картинки ──");
        IOSPYDiag(@"• твик arm64, SpringBoard arm64e → ElleKit часто молча пропускает");
        IOSPYDiag(@"• инжект dlopen из демона — запасной путь, не всегда работает");
        IOSPYDiag(@"• кадр идёт через CARenderServer из bootstrap SpringBoard");
        if (!renderFn) {
            IOSPYDiag(@"✗ нет CARenderServerRenderDisplay — пакет сломан");
        } else if (renderClient == 0) {
            IOSPYDiag(@"✗ нет порта CARenderServer — task_for_pid / bootstrap");
        } else if ([cap containsString:@"no jpeg"] || [cap containsString:@"0x0"]) {
            IOSPYDiag(@"✗ рендер есть, но JPEG пустой — демон не видит экран (iOS 16+)");
        }
    } else if (tweakSock) {
        IOSPYDiag(@"OK путь через твик в SpringBoard — основной режим");
    }
}
