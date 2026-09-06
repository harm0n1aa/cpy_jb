#import <Foundation/Foundation.h>

@interface IOSPYDaemonCapture : NSObject
+ (instancetype)shared;
- (void)start;
- (void)stop;
@end
