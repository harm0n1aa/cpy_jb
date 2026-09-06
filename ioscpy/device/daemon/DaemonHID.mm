#import "DaemonHID.h"

#import <CoreFoundation/CoreFoundation.h>
#import <mach/mach_time.h>
#import <arpa/inet.h>
#import <math.h>
#import <string.h>

#if __has_include(<IOKit/IOKitLib.h>)
#import <IOKit/IOKitLib.h>
#define IOSPY_HAS_IOKITLIB 1
#ifndef IORegistryEntryGetRegistryEntryID
extern "C" kern_return_t IORegistryEntryGetRegistryEntryID(io_registry_entry_t entry, uint64_t *entryID);
#endif
#endif

typedef uint32_t IOHIDDigitizerTransducerType;
typedef double IOHIDFloat;
typedef uint32_t IOHIDEventField;
typedef uint32_t IOOptionBits;
typedef struct __IOHIDEvent *IOHIDEventRef;
typedef struct __IOHIDEventSystemClient *IOHIDEventSystemClientRef;
typedef struct __IOHIDServiceClient *IOHIDServiceClientRef;

extern "C" {
IOHIDEventRef IOHIDEventCreateDigitizerEvent(CFAllocatorRef allocator, uint64_t timeStamp,
                                             IOHIDDigitizerTransducerType type, uint32_t index,
                                             uint32_t identity, uint32_t eventMask,
                                             uint32_t buttonMask, IOHIDFloat x, IOHIDFloat y,
                                             IOHIDFloat z, IOHIDFloat tipPressure,
                                             IOHIDFloat barrelPressure, Boolean range, Boolean touch,
                                             IOOptionBits options);
IOHIDEventRef IOHIDEventCreateDigitizerFingerEvent(CFAllocatorRef allocator, uint64_t timeStamp,
                                                   uint32_t index, uint32_t identity,
                                                   uint32_t eventMask, IOHIDFloat x, IOHIDFloat y,
                                                   IOHIDFloat z, IOHIDFloat tipPressure,
                                                   IOHIDFloat twist, Boolean range, Boolean touch,
                                                   IOOptionBits options);
void IOHIDEventAppendEvent(IOHIDEventRef parent, IOHIDEventRef child);
void IOHIDEventSetIntegerValue(IOHIDEventRef e, IOHIDEventField f, int v);
void IOHIDEventSetFloatValue(IOHIDEventRef e, IOHIDEventField f, IOHIDFloat v);
void IOHIDEventSetSenderID(IOHIDEventRef e, uint64_t senderID);
uint64_t IOHIDEventGetSenderID(IOHIDEventRef e);
uint32_t IOHIDEventGetType(IOHIDEventRef e);
IOHIDEventRef IOHIDEventCreateKeyboardEvent(CFAllocatorRef allocator, uint64_t timeStamp,
                                            uint32_t usagePage, uint32_t usage, Boolean down,
                                            IOOptionBits options);
IOHIDEventSystemClientRef IOHIDEventSystemClientCreate(CFAllocatorRef allocator);
void IOHIDEventSystemClientDispatchEvent(IOHIDEventSystemClientRef client, IOHIDEventRef event);
void IOHIDEventSystemClientScheduleWithRunLoop(IOHIDEventSystemClientRef, CFRunLoopRef, CFStringRef);
void IOHIDEventSystemClientRegisterEventCallback(IOHIDEventSystemClientRef, void *cb, void *target,
                                                 void *refcon);
void IOHIDEventSystemClientSetMatching(IOHIDEventSystemClientRef client, CFDictionaryRef matching);
CFArrayRef IOHIDEventSystemClientCopyServices(IOHIDEventSystemClientRef client);
uint64_t IOHIDServiceClientGetRegistryID(IOHIDServiceClientRef service);
}

#define kIOHIDDigitizerEventRange 0x00000001u
#define kIOHIDDigitizerEventTouch 0x00000002u
#define kIOHIDDigitizerEventPosition 0x00000004u
#define kIOHIDDigitizerEventIdentity 0x00000020u
#define kIOHIDDigitizerTransducerTypeHand 3
#define kIOHIDEventTypeDigitizer 11
#define kFieldDigitizerIsDisplayInteg 0x000b0019
#define kFieldDigitizerEventMask 0x000b0007
#define kFieldDigitizerRange 0x000b0008
#define kFieldDigitizerTouch 0x000b0009
#define kFieldDigitizerMajorRadius 0x000b0014
#define kFieldDigitizerMinorRadius 0x000b0015

static uint64_t gSenderID = 0;
static IOHIDEventSystemClientRef gClient = NULL;

static void adoptSenderID(uint64_t sid) {
    if (sid != 0 && sid != gSenderID) {
        gSenderID = sid;
        NSLog(@"[ioscpyd] HID senderID 0x%llx", sid);
    }
}

static void senderCallback(void *target, void *refcon, void *service, IOHIDEventRef event) {
    (void)target;
    (void)refcon;
    (void)service;
    if (IOHIDEventGetType(event) == kIOHIDEventTypeDigitizer) {
        adoptSenderID(IOHIDEventGetSenderID(event));
    }
}

static uint64_t senderIDFromHIDServices(IOHIDEventSystemClientRef client) {
    if (!client) {
        return 0;
    }
    const uint32_t usages[] = {0x04, 0x05, 0x01};
    for (size_t i = 0; i < sizeof(usages) / sizeof(usages[0]); i++) {
        NSDictionary *match = @{@"PrimaryUsagePage": @(0x0D), @"PrimaryUsage": @(usages[i])};
        IOHIDEventSystemClientSetMatching(client, (__bridge CFDictionaryRef)match);
        CFArrayRef svcs = IOHIDEventSystemClientCopyServices(client);
        if (!svcs) {
            continue;
        }
        uint64_t found = 0;
        CFIndex n = CFArrayGetCount(svcs);
        for (CFIndex j = 0; j < n; j++) {
            IOHIDServiceClientRef s = (IOHIDServiceClientRef)CFArrayGetValueAtIndex(svcs, j);
            uint64_t rid = IOHIDServiceClientGetRegistryID(s);
            if (rid != 0) {
                found = rid;
                break;
            }
        }
        CFRelease(svcs);
        if (found) {
            return found;
        }
    }
    return 0;
}

#if IOSPY_HAS_IOKITLIB
static uint64_t senderIDFromIORegistry(void) {
    static const char *classes[] = {
        "AppleMultitouchHIDService",
        "AppleMultitouchDigitizerHIDEventDriver",
        "AppleMultitouchDevice",
        NULL,
    };
#ifdef kIOMainPortDefault
    mach_port_t master = kIOMainPortDefault;
#elif defined(kIOMasterPortDefault)
    mach_port_t master = kIOMasterPortDefault;
#else
    mach_port_t master = 0;
#endif
    for (const char **name = classes; *name; name++) {
        io_iterator_t it = 0;
        if (IOServiceGetMatchingServices(master, IOServiceMatching(*name), &it) != KERN_SUCCESS) {
            continue;
        }
        uint64_t found = 0;
        io_object_t svc;
        while ((svc = IOIteratorNext(it))) {
            uint64_t rid = 0;
            if (IORegistryEntryGetRegistryEntryID(svc, &rid) == KERN_SUCCESS && rid != 0) {
                found = rid;
            }
            IOObjectRelease(svc);
            if (found) {
                break;
            }
        }
        IOObjectRelease(it);
        if (found) {
            return found;
        }
    }
    return 0;
}
#endif

static void hidInit(void) {
    if (gClient) {
        return;
    }
    gClient = IOHIDEventSystemClientCreate(kCFAllocatorDefault);
    if (!gClient) {
        NSLog(@"[ioscpyd] IOHIDEventSystemClientCreate returned NULL");
        return;
    }
    IOHIDEventSystemClientRef monitor = IOHIDEventSystemClientCreate(kCFAllocatorDefault);
    if (monitor) {
        IOHIDEventSystemClientRegisterEventCallback(monitor, (void *)senderCallback, NULL, NULL);
        IOHIDEventSystemClientScheduleWithRunLoop(monitor, CFRunLoopGetMain(), kCFRunLoopDefaultMode);
        adoptSenderID(senderIDFromHIDServices(monitor));
    }
    if (gSenderID == 0) {
        adoptSenderID(senderIDFromHIDServices(gClient));
    }
#if IOSPY_HAS_IOKITLIB
    if (gSenderID == 0) {
        adoptSenderID(senderIDFromIORegistry());
    }
#endif
}

void IOSPYDaemonHIDStart(void) {
    hidInit();
}

static void injectTouch(uint8_t phase, uint8_t fingerID, float x, float y) {
    hidInit();
    if (!gClient) {
        return;
    }
    x = fmaxf(0.0f, fminf(1.0f, x));
    y = fmaxf(0.0f, fminf(1.0f, y));
    uint64_t ts = mach_absolute_time();
    Boolean touch = (phase != 2);
    Boolean range = touch;
    uint32_t mask = (phase == 1) ? kIOHIDDigitizerEventPosition
                                 : (kIOHIDDigitizerEventRange | kIOHIDDigitizerEventTouch);
    IOHIDEventRef parent = IOHIDEventCreateDigitizerEvent(
        kCFAllocatorDefault, ts, kIOHIDDigitizerTransducerTypeHand, 0, 0, mask, 0, 0, 0, 0, 0, 0,
        range, touch, 0);
    if (!parent) {
        return;
    }
    IOHIDEventSetIntegerValue(parent, kFieldDigitizerIsDisplayInteg, 1);
    IOHIDEventRef finger = IOHIDEventCreateDigitizerFingerEvent(
        kCFAllocatorDefault, ts, (uint32_t)fingerID, (uint32_t)fingerID + 1, mask, (IOHIDFloat)x,
        (IOHIDFloat)y, 0, touch ? 1.0 : 0.0, 0, range, touch, 0);
    if (finger) {
        IOHIDEventSetFloatValue(finger, kFieldDigitizerMajorRadius, 0.04f);
        IOHIDEventSetFloatValue(finger, kFieldDigitizerMinorRadius, 0.04f);
        IOHIDEventAppendEvent(parent, finger);
    }
    uint32_t parentMask = (phase == 1) ? kIOHIDDigitizerEventPosition
                                       : (kIOHIDDigitizerEventRange | kIOHIDDigitizerEventTouch |
                                          kIOHIDDigitizerEventIdentity);
    IOHIDEventSetIntegerValue(parent, kFieldDigitizerEventMask, parentMask);
    IOHIDEventSetIntegerValue(parent, kFieldDigitizerRange, range ? 1 : 0);
    IOHIDEventSetIntegerValue(parent, kFieldDigitizerTouch, touch ? 1 : 0);
    if (gSenderID != 0) {
        IOHIDEventSetSenderID(parent, gSenderID);
        if (finger) {
            IOHIDEventSetSenderID(finger, gSenderID);
        }
    }
    IOHIDEventSystemClientDispatchEvent(gClient, parent);
    if (finger) {
        CFRelease(finger);
    }
    CFRelease(parent);
}

static void injectButton(uint32_t usagePage, uint32_t usage) {
    hidInit();
    if (!gClient) {
        return;
    }
    for (int down = 1; down >= 0; down--) {
        IOHIDEventRef e = IOHIDEventCreateKeyboardEvent(kCFAllocatorDefault, mach_absolute_time(),
                                                        usagePage, usage, down ? true : false, 0);
        if (!e) {
            continue;
        }
        if (gSenderID != 0) {
            IOHIDEventSetSenderID(e, gSenderID);
        }
        IOHIDEventSystemClientDispatchEvent(gClient, e);
        CFRelease(e);
    }
}

void IOSPYDaemonHandleTouchPayload(NSData *payload) {
    if (payload.length < 10) {
        return;
    }
    const uint8_t *b = (const uint8_t *)payload.bytes;
    uint8_t phase = b[0];
    uint8_t fingerID = b[1];
    uint32_t xb, yb;
    memcpy(&xb, b + 2, 4);
    memcpy(&yb, b + 6, 4);
    xb = ntohl(xb);
    yb = ntohl(yb);
    float x, y;
    memcpy(&x, &xb, 4);
    memcpy(&y, &yb, 4);
    injectTouch(phase, fingerID, x, y);
}

void IOSPYDaemonHandleSystemActionPayload(NSData *payload) {
    if (payload.length < 2) {
        return;
    }
    uint16_t action;
    memcpy(&action, payload.bytes, 2);
    action = ntohs(action);
    switch (action) {
        case 1:
            injectButton(0x0C, 0x40);
            break;
        case 2:
            injectButton(0x0C, 0x30);
            break;
        case 3:
            injectButton(0x0C, 0x40);
            break;
        default:
            break;
    }
}
