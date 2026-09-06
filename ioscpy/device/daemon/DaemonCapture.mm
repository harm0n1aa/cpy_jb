#import "DaemonCapture.h"
#import "Capture.h"
#import "FrameStore.h"
#import "Protocol.h"

#import <arpa/inet.h>

static const CGFloat kMaxDimension = 1280.0;
static const CGFloat kQuality = 0.65;
static const NSTimeInterval kInterval = 1.0 / 12.0;

static NSData *packJpegFrame(int width, int height, NSData *jpeg) {
    NSMutableData *body = [NSMutableData dataWithCapacity:16 + jpeg.length];
    uint32_t w = htonl((uint32_t)width);
    uint32_t h = htonl((uint32_t)height);
    uint32_t fl = htonl(0);
    uint32_t len = htonl((uint32_t)jpeg.length);
    [body appendBytes:&w length:4];
    [body appendBytes:&h length:4];
    [body appendBytes:&fl length:4];
    [body appendBytes:&len length:4];
    [body appendData:jpeg];
    return body;
}

@implementation IOSPYDaemonCapture {
    dispatch_queue_t _queue;
    dispatch_source_t _timer;
    int _failures;
}

+ (instancetype)shared {
    static IOSPYDaemonCapture *cap = nil;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        cap = [[IOSPYDaemonCapture alloc] init];
    });
    return cap;
}

- (instancetype)init {
    if ((self = [super init])) {
        _queue = dispatch_queue_create("com.ioscpy.daemon.capture", DISPATCH_QUEUE_SERIAL);
    }
    return self;
}

- (void)start {
    dispatch_async(_queue, ^{
        if (self->_timer) {
            return;
        }
        self->_failures = 0;
        NSLog(@"[ioscpyd] daemon capture starting (tweak not attached)");
        self->_timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, self->_queue);
        uint64_t interval = (uint64_t)(kInterval * NSEC_PER_SEC);
        dispatch_source_set_timer(self->_timer, DISPATCH_TIME_NOW, interval, interval / 4);
        dispatch_source_set_event_handler(self->_timer, ^{ [self tick]; });
        dispatch_resume(self->_timer);
    });
}

- (void)stop {
    dispatch_async(_queue, ^{
        if (!self->_timer) {
            return;
        }
        dispatch_source_cancel(self->_timer);
        self->_timer = nil;
        NSLog(@"[ioscpyd] daemon capture stopped");
    });
}

- (void)tick {
    int width = 0, height = 0;
    NSData *jpeg = IOSPYCaptureScreenJPEG(kMaxDimension, kQuality, &width, &height, NULL, NULL);
    if (!jpeg || width < 2 || height < 2) {
        self->_failures++;
        if (self->_failures == 1 || self->_failures == 30 || (self->_failures % 120) == 0) {
            NSLog(@"[ioscpyd] daemon capture got no frame (n=%d)", self->_failures);
        }
        return;
    }
    self->_failures = 0;
    [[IOSPYFrameStore shared] setPayload:packJpegFrame(width, height, jpeg)];
}

@end
