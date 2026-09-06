#import "Capture.h"
#ifndef IOSPY_IN_DAEMON
#import <UIKit/UIKit.h>
#endif
#import <IOSurface/IOSurfaceRef.h>
#import <ImageIO/ImageIO.h>
#import <MobileCoreServices/MobileCoreServices.h>
#import <objc/message.h>
#import <objc/runtime.h>
#import <dlfcn.h>
#import <mach/mach.h>

#ifdef __cplusplus
extern "C" {
#endif
extern mach_port_t bootstrap_port;
kern_return_t bootstrap_look_up(mach_port_t bp, const char *service_name, mach_port_t *sp);
#ifdef __cplusplus
}
#endif

#ifndef MAX
#define MAX(a, b) (((a) > (b)) ? (a) : (b))
#endif

static NSString *gCaptureNote = @"";

NSString *IOSPYCaptureLastNote(void) {
    return gCaptureNote ?: @"";
}

static void setCaptureNote(NSString *msg) {
    gCaptureNote = msg ?: @"";
    NSLog(@"[ioscpy] capture: %@", gCaptureNote);
}

#ifdef IOSPY_IN_DAEMON
#import "SpringBoardLookup.h"
#endif

static NSData *jpegEncodeImage(CGImageRef image, CGFloat quality);

// UIKit private screen capture — works inside SpringBoard; usually empty from launchd.
static NSData *jpegFromUIKitPrivate(CGFloat quality, int *outWidth, int *outHeight) {
    dlopen("/System/Library/Frameworks/UIKit.framework/UIKit", RTLD_LAZY);
    const char *syms[] = {"_UICreateScreenUIImage", "UIGetScreenImage", "UICreateScreenImage", NULL};
    typedef CGImageRef (*UiFn)(void);
    for (const char **s = syms; *s; s++) {
        UiFn fn = (UiFn)dlsym(RTLD_DEFAULT, *s);
        if (!fn) {
            continue;
        }
        CGImageRef img = fn();
        if (!img) {
            continue;
        }
        int tw = (int)CGImageGetWidth(img);
        int th = (int)CGImageGetHeight(img);
        NSData *jpeg = jpegEncodeImage(img, quality);
        CGImageRelease(img);
        if (jpeg && tw > 1 && th > 1) {
            if (outWidth) {
                *outWidth = tw;
            }
            if (outHeight) {
                *outHeight = th;
            }
            NSLog(@"[ioscpy] screenshot via %s %dx%d", *s, tw, th);
            return jpeg;
        }
    }
    return nil;
}

#ifndef IOSPY_IN_DAEMON
// Draw the key window hierarchy — reliable inside SpringBoard when CARenderServer
// returns an empty surface (common on iOS 16+ with some display names).
static NSData *jpegFromKeyWindow(CGFloat quality, int *outWidth, int *outHeight) {
    __block NSData *out = nil;
    __block int w = 0, h = 0;
    void (^snap)(void) = ^{
        UIApplication *app = [UIApplication sharedApplication];
        if (!app) {
            return;
        }
        UIWindow *win = nil;
        for (UIWindow *candidate in app.windows) {
            if (candidate.isKeyWindow) {
                win = candidate;
                break;
            }
        }
        if (!win) {
            win = app.windows.firstObject;
        }
        if (!win || win.bounds.size.width < 2 || win.bounds.size.height < 2) {
            return;
        }
        CGFloat scale = [UIScreen mainScreen].scale;
        if (scale < 1) {
            scale = 1;
        }
        UIGraphicsBeginImageContextWithOptions(win.bounds.size, YES, scale);
        BOOL ok = [win drawViewHierarchyInRect:win.bounds afterScreenUpdates:NO];
        UIImage *img = UIGraphicsGetImageFromCurrentImageContext();
        UIGraphicsEndImageContext();
        if (!ok || !img.CGImage) {
            return;
        }
        w = (int)CGImageGetWidth(img.CGImage);
        h = (int)CGImageGetHeight(img.CGImage);
        out = jpegEncodeImage(img.CGImage, quality);
    };
    if ([NSThread isMainThread]) {
        snap();
    } else {
        dispatch_sync(dispatch_get_main_queue(), snap);
    }
    if (out && outWidth) {
        *outWidth = w;
    }
    if (out && outHeight) {
        *outHeight = h;
    }
    return out;
}
#endif

static int collectRenderClients(uint32_t *ids, int max) {
    int n = 0;
#ifdef IOSPY_IN_DAEMON
    uint32_t sb = 0;
    if (IOSPYSpringBoardRenderClient(&sb) && n < max) {
        ids[n++] = sb;
    }
#endif
    if (n < max) {
        ids[n++] = 0;
    }
    const char *names[] = {"com.apple.CARenderServer", "com.apple.windowserver.active", NULL};
    for (const char **s = names; *s && n < max; s++) {
        mach_port_t port = MACH_PORT_NULL;
        if (bootstrap_look_up(bootstrap_port, *s, &port) == KERN_SUCCESS && MACH_PORT_VALID(port)) {
            ids[n++] = (uint32_t)port;
        }
    }
    if (n < max) {
        ids[n++] = 0xFFE;
    }
    return n;
}

typedef void (*CARenderServerRenderDisplayFn)(uint32_t client, CFStringRef display,
                                               IOSurfaceRef surface, int x, int y);
typedef int (*IOMobileFramebufferGetMainDisplayFn)(void **fb);
typedef int (*IOMobileFramebufferGetLayerDefaultSurfaceFn)(void *fb, int layer,
                                                           IOSurfaceRef *surface);

static CARenderServerRenderDisplayFn renderDisplayFn(void) {
    static CARenderServerRenderDisplayFn fn = NULL;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        fn = (CARenderServerRenderDisplayFn)dlsym(RTLD_DEFAULT, "CARenderServerRenderDisplay");
        if (!fn) {
            const char *paths[] = {
                "/System/Library/Frameworks/QuartzCore.framework/QuartzCore",
                "/System/Library/PrivateFrameworks/QuartzCore.framework/QuartzCore",
                NULL,
            };
            for (const char **p = paths; *p; p++) {
                void *h = dlopen(*p, RTLD_LAZY);
                if (!h) {
                    continue;
                }
                fn = (CARenderServerRenderDisplayFn)dlsym(h, "CARenderServerRenderDisplay");
                if (fn) {
                    break;
                }
            }
        }
    });
    return fn;
}

static IOSurfaceRef mainFramebufferSurface(int *outWidth, int *outHeight) {
    static IOMobileFramebufferGetMainDisplayFn getMain = NULL;
    static IOMobileFramebufferGetLayerDefaultSurfaceFn getSurf = NULL;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        void *h = dlopen(
            "/System/Library/PrivateFrameworks/IOMobileFramebuffer.framework/IOMobileFramebuffer",
            RTLD_LAZY);
        if (h) {
            getMain = (IOMobileFramebufferGetMainDisplayFn)dlsym(h, "IOMobileFramebufferGetMainDisplay");
            getSurf =
                (IOMobileFramebufferGetLayerDefaultSurfaceFn)dlsym(h, "IOMobileFramebufferGetLayerDefaultSurface");
        }
    });
    if (!getMain || !getSurf) {
        return NULL;
    }
    void *fb = NULL;
    if (getMain(&fb) != 0 || !fb) {
        return NULL;
    }
    IOSurfaceRef surf = NULL;
    for (int layer = 0; layer <= 3; layer++) {
        IOSurfaceRef candidate = NULL;
        if (getSurf(fb, layer, &candidate) == 0 && candidate) {
            surf = candidate;
            break;
        }
    }
    if (!surf) {
        return NULL;
    }
    if (outWidth) {
        *outWidth = (int)IOSurfaceGetWidth(surf);
    }
    if (outHeight) {
        *outHeight = (int)IOSurfaceGetHeight(surf);
    }
    return surf;
}

BOOL IOSPYCaptureAvailable(void) {
    int w = 0, h = 0;
    return renderDisplayFn() != NULL || mainFramebufferSurface(&w, &h) != NULL;
}

#ifdef IOSPY_IN_DAEMON
BOOL IOSPYCaptureHasRenderFn(void) {
    return renderDisplayFn() != NULL;
}

void IOSPYCaptureProbeFramebuffer(int *outWidth, int *outHeight) {
    mainFramebufferSurface(outWidth, outHeight);
}
#endif

static double nowMs(void) {
    return CFAbsoluteTimeGetCurrent() * 1000.0;
}

// QuartzCore window-server size. Safe from a launchd daemon (no UIKit).
static CGSize windowServerPixelSize(void) {
    Class cls = NSClassFromString(@"CAWindowServer");
    if (!cls) {
        return CGSizeZero;
    }
    SEL running = sel_registerName("serverIfRunning");
    if (![cls respondsToSelector:running]) {
        return CGSizeZero;
    }
    id server = ((id (*)(id, SEL))objc_msgSend)(cls, running);
    if (!server) {
        return CGSizeZero;
    }
    NSArray *displays = [server valueForKey:@"displays"];
    id display = displays.firstObject;
    if (!display) {
        return CGSizeZero;
    }
    CGSize sz = CGSizeZero;
    id native = [display valueForKey:@"nativeSize"];
    if ([native isKindOfClass:[NSValue class]]) {
        [(NSValue *)native getValue:&sz];
    }
    if (sz.width < 2 || sz.height < 2) {
        id boundsVal = [display valueForKey:@"bounds"];
        CGRect bounds = CGRectZero;
        if ([boundsVal isKindOfClass:[NSValue class]]) {
            [(NSValue *)boundsVal getValue:&bounds];
        }
        CGFloat scale = 1;
        id scaleVal = [display valueForKey:@"scale"];
        if ([scaleVal respondsToSelector:@selector(doubleValue)]) {
            scale = (CGFloat)[scaleVal doubleValue];
            if (scale < 1) {
                scale = 1;
            }
        }
        sz = CGSizeMake(bounds.size.width * scale, bounds.size.height * scale);
    }
    return sz;
}

// Native screen size in pixels (cached; doesn't change at runtime).
static CGSize nativeScreenSize(void) {
    static CGSize size = {0, 0};
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        int w = 0, h = 0;
        if (mainFramebufferSurface(&w, &h) && w > 0 && h > 0) {
            size = CGSizeMake(w, h);
            return;
        }
        CGSize ws = windowServerPixelSize();
        if (ws.width > 1 && ws.height > 1) {
            size = ws;
            return;
        }
#ifdef IOSPY_IN_DAEMON
        size = CGSizeZero;
#else
        size = [UIScreen mainScreen].nativeBounds.size;
#endif
    });
    return size;
}

// The render server addresses displays by name; the main display's name varies
// by device, so read it rather than hardcoding "LCD".
static CFStringRef mainDisplayName(void) {
    static CFStringRef name = NULL;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        NSString *resolved = nil;
        Class displayClass = NSClassFromString(@"CADisplay");
        if (displayClass && [displayClass respondsToSelector:@selector(mainDisplay)]) {
            id display = [displayClass performSelector:@selector(mainDisplay)];
            if ([display respondsToSelector:@selector(name)]) {
                resolved = [display performSelector:@selector(name)];
            }
        }
        if (resolved.length == 0) {
            resolved = @"LCD";
        }
        name = (CFStringRef)CFBridgingRetain(resolved);
    });
    return name;
}

// Reusable destination surface, recreated only when the target size changes.
static IOSurfaceRef surfaceForSize(int width, int height) {
    static IOSurfaceRef surface = NULL;
    static int cachedW = 0, cachedH = 0;
    if (surface && cachedW == width && cachedH == height) {
        return surface;
    }
    if (surface) {
        CFRelease(surface);
        surface = NULL;
    }
    int bpr = width * 4;
    NSDictionary *props = @{
        (id)kIOSurfaceWidth: @(width),
        (id)kIOSurfaceHeight: @(height),
        (id)kIOSurfaceBytesPerRow: @(bpr),
        (id)kIOSurfaceBytesPerElement: @4,
        (id)kIOSurfaceAllocSize: @(bpr * height),
        (id)kIOSurfacePixelFormat: @((uint32_t)'BGRA'),
        @"IOSurfaceIsGlobal": @YES,
    };
    surface = IOSurfaceCreate((__bridge CFDictionaryRef)props);
    cachedW = width;
    cachedH = height;
    return surface;
}

// Destination surface for the H.264 path, recreated only when the target size
// changes. Kept separate from the native source surface above.
static IOSurfaceRef destSurfaceForSize(int width, int height) {
    static IOSurfaceRef surface = NULL;
    static int cachedW = 0, cachedH = 0;
    if (surface && cachedW == width && cachedH == height) {
        return surface;
    }
    if (surface) {
        CFRelease(surface);
        surface = NULL;
    }
    int bpr = width * 4;
    NSDictionary *props = @{
        (id)kIOSurfaceWidth: @(width),
        (id)kIOSurfaceHeight: @(height),
        (id)kIOSurfaceBytesPerRow: @(bpr),
        (id)kIOSurfaceBytesPerElement: @4,
        (id)kIOSurfaceAllocSize: @(bpr * height),
        (id)kIOSurfacePixelFormat: @((uint32_t)'BGRA'),
        @"IOSurfaceIsGlobal": @YES,
    };
    surface = IOSurfaceCreate((__bridge CFDictionaryRef)props);
    cachedW = width;
    cachedH = height;
    return surface;
}

IOSurfaceRef IOSPYCaptureScreenSurface(CGFloat maxDimension, int *outWidth, int *outHeight) {
    @autoreleasepool {
        CARenderServerRenderDisplayFn render = renderDisplayFn();
        if (!render) {
            return NULL;
        }
        CGSize native = nativeScreenSize();
        if (native.width < 1 || native.height < 1) {
            return NULL;
        }
        int nw = (int)native.width;
        int nh = (int)native.height;

        CGSize target = native;
        if (maxDimension > 0) {
            CGFloat longest = MAX(native.width, native.height);
            if (longest > maxDimension) {
                CGFloat factor = maxDimension / longest;
                target = CGSizeMake(round(native.width * factor), round(native.height * factor));
            }
        }
        // Round down to even dimensions for 4:2:0 H.264.
        int tw = ((int)target.width) & ~1;
        int th = ((int)target.height) & ~1;
        if (tw < 2) tw = 2;
        if (th < 2) th = 2;

        IOSurfaceRef src = surfaceForSize(nw, nh);
        IOSurfaceRef dst = destSurfaceForSize(tw, th);
        if (!src || !dst) {
            return NULL;
        }

        const uint32_t bgra = kCGImageAlphaNoneSkipFirst | kCGBitmapByteOrder32Little;
        CGColorSpaceRef space = CGColorSpaceCreateDeviceRGB();
        render(0, mainDisplayName(), src, 0, 0);

        IOSurfaceLock(src, kIOSurfaceLockReadOnly, NULL);
        void *base = IOSurfaceGetBaseAddress(src);
        size_t bytesPerRow = IOSurfaceGetBytesPerRow(src);
        CGDataProviderRef provider = CGDataProviderCreateWithData(NULL, base, bytesPerRow * nh, NULL);
        CGImageRef nativeImage = CGImageCreate(nw, nh, 8, 32, bytesPerRow, space, bgra, provider,
                                               NULL, false, kCGRenderingIntentDefault);

        BOOL ok = NO;
        if (nativeImage) {
            IOSurfaceLock(dst, 0, NULL);
            void *dbase = IOSurfaceGetBaseAddress(dst);
            size_t dbpr = IOSurfaceGetBytesPerRow(dst);
            CGContextRef ctx = CGBitmapContextCreate(dbase, tw, th, 8, dbpr, space, bgra);
            if (ctx) {
                CGContextSetInterpolationQuality(ctx, kCGInterpolationLow);
                CGContextDrawImage(ctx, CGRectMake(0, 0, tw, th), nativeImage);
                CGContextRelease(ctx);
                ok = YES;
            }
            IOSurfaceUnlock(dst, 0, NULL);
        }

        CGImageRelease(nativeImage);
        CGDataProviderRelease(provider);
        IOSurfaceUnlock(src, kIOSurfaceLockReadOnly, NULL);
        CGColorSpaceRelease(space);

        if (!ok) {
            return NULL;
        }
        if (outWidth) {
            *outWidth = tw;
        }
        if (outHeight) {
            *outHeight = th;
        }
        return dst;
    }
}

static NSData *jpegEncodeImage(CGImageRef image, CGFloat quality) {
    if (!image) {
        return nil;
    }
    NSMutableData *data = [NSMutableData data];
    CGImageDestinationRef dest =
        CGImageDestinationCreateWithData((__bridge CFMutableDataRef)data, CFSTR("public.jpeg"), 1, NULL);
    if (!dest) {
        return nil;
    }
    NSDictionary *options = @{(__bridge id)kCGImageDestinationLossyCompressionQuality: @(quality)};
    CGImageDestinationAddImage(dest, image, (__bridge CFDictionaryRef)options);
    BOOL ok = CGImageDestinationFinalize(dest);
    CFRelease(dest);
    return ok ? data : nil;
}

static NSData *jpegFromFramebuffer(CGFloat maxDimension, CGFloat quality, int *outWidth, int *outHeight,
                                   double *outRenderMs, double *outEncodeMs) {
    int nw = 0, nh = 0;
    IOSurfaceRef surface = mainFramebufferSurface(&nw, &nh);
    if (!surface || nw < 2 || nh < 2) {
        return nil;
    }
    double renderStart = nowMs();
    IOSurfaceLock(surface, kIOSurfaceLockReadOnly, NULL);
    void *base = IOSurfaceGetBaseAddress(surface);
    size_t bytesPerRow = IOSurfaceGetBytesPerRow(surface);
    CGColorSpaceRef space = CGColorSpaceCreateDeviceRGB();
    const uint32_t bgra = kCGImageAlphaNoneSkipFirst | kCGBitmapByteOrder32Little;
    CGDataProviderRef provider = CGDataProviderCreateWithData(NULL, base, bytesPerRow * nh, NULL);
    CGImageRef nativeImage = CGImageCreate(nw, nh, 8, 32, bytesPerRow, space, bgra, provider, NULL,
                                           false, kCGRenderingIntentDefault);
    CGFloat longest = (CGFloat)MAX(nw, nh);
    int tw = nw, th = nh;
    if (maxDimension > 0 && longest > maxDimension) {
        CGFloat factor = maxDimension / longest;
        tw = (int)round(nw * factor);
        th = (int)round(nh * factor);
    }
    CGImageRef image = nativeImage;
    CGContextRef ctx = NULL;
    if (tw != nw || th != nh) {
        ctx = CGBitmapContextCreate(NULL, tw, th, 8, 0, space, bgra);
        if (ctx && nativeImage) {
            CGContextSetInterpolationQuality(ctx, kCGInterpolationLow);
            CGContextDrawImage(ctx, CGRectMake(0, 0, tw, th), nativeImage);
            CGImageRef scaled = CGBitmapContextCreateImage(ctx);
            CGImageRelease(nativeImage);
            image = scaled;
        }
    }
    if (outRenderMs) {
        *outRenderMs = nowMs() - renderStart;
    }
    double encodeStart = nowMs();
    NSData *jpeg = jpegEncodeImage(image, quality);
    if (outEncodeMs) {
        *outEncodeMs = nowMs() - encodeStart;
    }
    CGImageRelease(image);
    if (ctx) {
        CGContextRelease(ctx);
    }
    CGDataProviderRelease(provider);
    IOSurfaceUnlock(surface, kIOSurfaceLockReadOnly, NULL);
    CGColorSpaceRelease(space);
    if (!jpeg) {
        return nil;
    }
    if (outWidth) {
        *outWidth = tw;
    }
    if (outHeight) {
        *outHeight = th;
    }
    return jpeg;
}

static NSData *jpegFromRender(CARenderServerRenderDisplayFn render, CFStringRef display, int nw,
                              int nh, CGFloat maxDimension, CGFloat quality, int *outWidth,
                              int *outHeight, double *outRenderMs, double *outEncodeMs) {
    if (!render || nw < 2 || nh < 2) {
        return nil;
    }
    CGSize native = CGSizeMake(nw, nh);
    CGSize target = native;
    if (maxDimension > 0) {
        CGFloat longest = MAX(native.width, native.height);
        if (longest > maxDimension) {
            CGFloat factor = maxDimension / longest;
            target = CGSizeMake(round(native.width * factor), round(native.height * factor));
        }
    }
    int tw = (int)target.width;
    int th = (int)target.height;
    IOSurfaceRef surface = surfaceForSize(nw, nh);
    if (!surface) {
        return nil;
    }
    const uint32_t bgra = kCGImageAlphaNoneSkipFirst | kCGBitmapByteOrder32Little;
    CGColorSpaceRef space = CGColorSpaceCreateDeviceRGB();
    double renderStart = nowMs();
    uint32_t clients[6];
    int nClients = collectRenderClients(clients, 6);
    BOOL painted = NO;
    for (int ci = 0; ci < nClients && !painted; ci++) {
        render(clients[ci], display, surface, 0, 0);
        IOSurfaceLock(surface, kIOSurfaceLockReadOnly, NULL);
        void *probe = IOSurfaceGetBaseAddress(surface);
        size_t bpr = IOSurfaceGetBytesPerRow(surface);
        if (probe && nw > 2 && nh > 2) {
            uint32_t *p = (uint32_t *)probe;
            if (!(p[0] == 0 && p[(bpr / 4) * (nh / 2) + (nw / 2)] == 0 &&
                  p[(bpr / 4) * (nh - 1) + (nw - 1)] == 0)) {
                painted = YES;
            }
        }
        IOSurfaceUnlock(surface, kIOSurfaceLockReadOnly, NULL);
    }
    if (!painted) {
        CGColorSpaceRelease(space);
        return nil;
    }
    IOSurfaceLock(surface, kIOSurfaceLockReadOnly, NULL);
    void *base = IOSurfaceGetBaseAddress(surface);
    size_t bytesPerRow = IOSurfaceGetBytesPerRow(surface);
    CGDataProviderRef provider = CGDataProviderCreateWithData(NULL, base, bytesPerRow * nh, NULL);
    CGImageRef nativeImage = CGImageCreate(nw, nh, 8, 32, bytesPerRow, space, bgra, provider, NULL,
                                           false, kCGRenderingIntentDefault);
    CGContextRef ctx = CGBitmapContextCreate(NULL, tw, th, 8, 0, space, bgra);
    CGImageRef image = NULL;
    if (ctx && nativeImage) {
        CGContextSetInterpolationQuality(ctx, kCGInterpolationLow);
        CGContextDrawImage(ctx, CGRectMake(0, 0, tw, th), nativeImage);
        image = CGBitmapContextCreateImage(ctx);
    }
    if (ctx) {
        CGContextRelease(ctx);
    }
    CGImageRelease(nativeImage);
    CGDataProviderRelease(provider);
    IOSurfaceUnlock(surface, kIOSurfaceLockReadOnly, NULL);
    CGColorSpaceRelease(space);
    if (outRenderMs) {
        *outRenderMs = nowMs() - renderStart;
    }
    if (!image) {
        return nil;
    }
    double encodeStart = nowMs();
    NSData *jpeg = jpegEncodeImage(image, quality);
    CGImageRelease(image);
    if (outEncodeMs) {
        *outEncodeMs = nowMs() - encodeStart;
    }
    if (!jpeg) {
        return nil;
    }
    if (outWidth) {
        *outWidth = tw;
    }
    if (outHeight) {
        *outHeight = th;
    }
    return jpeg;
}

#ifdef IOSPY_IN_DAEMON
static NSData *jpegFromSpringBoardServices(CGFloat quality, int *outWidth, int *outHeight) {
    void *h = dlopen(
        "/System/Library/PrivateFrameworks/SpringBoardServices.framework/SpringBoardServices",
        RTLD_LAZY);
    if (!h) {
        h = dlopen("/System/Library/PrivateFrameworks/ScreenshotServices.framework/ScreenshotServices",
                   RTLD_LAZY);
    }
    const char *syms[] = {"SBSCopyScreenUIImage", "SBSScreenshotCopyImage", "_SBSCreateScreenshotImage",
                          "SBSCreateImageFromIOSurface", NULL};
    typedef CGImageRef (*SbsFn)(void);
    for (const char **s = syms; *s; s++) {
        SbsFn fn = (SbsFn)dlsym(h ? h : RTLD_DEFAULT, *s);
        if (!fn) {
            fn = (SbsFn)dlsym(RTLD_DEFAULT, *s);
        }
        if (!fn) {
            continue;
        }
        CGImageRef img = fn();
        if (!img) {
            continue;
        }
        int tw = (int)CGImageGetWidth(img);
        int th = (int)CGImageGetHeight(img);
        NSData *jpeg = jpegEncodeImage(img, quality);
        CGImageRelease(img);
        if (jpeg && tw > 1 && th > 1) {
            if (outWidth) {
                *outWidth = tw;
            }
            if (outHeight) {
                *outHeight = th;
            }
            NSLog(@"[ioscpyd] screenshot via %s %dx%d", *s, tw, th);
            return jpeg;
        }
    }
    return nil;
}
#endif

NSData *IOSPYCaptureScreenJPEG(CGFloat maxDimension, CGFloat quality,
                               int *outWidth, int *outHeight,
                               double *outRenderMs, double *outEncodeMs) {
    @autoreleasepool {
#ifdef IOSPY_IN_DAEMON
        NSData *ui = jpegFromUIKitPrivate(quality, outWidth, outHeight);
        if (ui) {
            setCaptureNote(@"uikit");
            return ui;
        }
        NSData *sbs = jpegFromSpringBoardServices(quality, outWidth, outHeight);
        if (sbs) {
            setCaptureNote(@"sbs");
            return sbs;
        }
        NSData *fb = jpegFromFramebuffer(maxDimension, quality, outWidth, outHeight, outRenderMs,
                                         outEncodeMs);
        if (fb) {
            setCaptureNote(@"framebuffer");
            return fb;
        }
        CARenderServerRenderDisplayFn render = renderDisplayFn();
        if (!render) {
            setCaptureNote(@"CARenderServerRenderDisplay missing");
            return nil;
        }
        CGSize native = nativeScreenSize();
        if (native.width > 1 && native.height > 1) {
            NSData *jpeg = jpegFromRender(render, mainDisplayName(), (int)native.width,
                                          (int)native.height, maxDimension, quality, outWidth,
                                          outHeight, outRenderMs, outEncodeMs);
            if (jpeg) {
                setCaptureNote(@"ok");
                return jpeg;
            }
        }
        CFStringRef names[] = {CFSTR("LCD"), CFSTR("Main"), CFSTR("built-in"), NULL};
        const int sizes[][2] = {{1170, 2532}, {828, 1792}, {1125, 2436}, {1284, 2778},
                                {1242, 2688}, {1080, 2340}, {750, 1624}, {0, 0}};
        for (CFStringRef *name = names; *name; name++) {
            for (int i = 0; sizes[i][0]; i++) {
                NSData *jpeg = jpegFromRender(render, *name, sizes[i][0], sizes[i][1], maxDimension,
                                              quality, outWidth, outHeight, outRenderMs, outEncodeMs);
                if (jpeg) {
                    setCaptureNote(@"ok");
                    return jpeg;
                }
            }
        }
        setCaptureNote([NSString stringWithFormat:@"no jpeg (size %.0fx%.0f render=%d)", native.width,
                                                  native.height, render != NULL]);
        return nil;
#else
        // Inside SpringBoard: try UIKit APIs first — CARenderServer often paints
        // an empty surface on iOS 16+ depending on display name / client id.
        NSData *ui = jpegFromUIKitPrivate(quality, outWidth, outHeight);
        if (ui) {
            setCaptureNote(@"tweak-uikit");
            return ui;
        }
        NSData *win = jpegFromKeyWindow(quality, outWidth, outHeight);
        if (win) {
            setCaptureNote(@"tweak-keywindow");
            return win;
        }
        CARenderServerRenderDisplayFn render = renderDisplayFn();
        if (!render) {
            NSData *fb = jpegFromFramebuffer(maxDimension, quality, outWidth, outHeight, outRenderMs,
                                             outEncodeMs);
            setCaptureNote(fb ? @"tweak-framebuffer" : @"tweak-no-backend");
            return fb;
        }

        CGSize native = nativeScreenSize();
        if (native.width < 1 || native.height < 1) {
            NSData *fb = jpegFromFramebuffer(maxDimension, quality, outWidth, outHeight, outRenderMs,
                                             outEncodeMs);
            setCaptureNote(fb ? @"tweak-framebuffer" : @"tweak-no-size");
            return fb;
        }
        CFStringRef names[] = {mainDisplayName(), CFSTR("LCD"), CFSTR("Main"), CFSTR("built-in"),
                               NULL};
        for (CFStringRef *name = names; *name; name++) {
            NSData *jpeg = jpegFromRender(render, *name, (int)native.width, (int)native.height,
                                          maxDimension, quality, outWidth, outHeight, outRenderMs,
                                          outEncodeMs);
            if (jpeg) {
                setCaptureNote(@"tweak-render");
                return jpeg;
            }
        }
        NSData *fb = jpegFromFramebuffer(maxDimension, quality, outWidth, outHeight, outRenderMs,
                                         outEncodeMs);
        setCaptureNote(fb ? @"tweak-framebuffer" : @"tweak-fail");
        return fb;
#endif
    }
}
