#import <Foundation/Foundation.h>

#ifdef __cplusplus
extern "C" {
#endif

void IOSPYDaemonHIDStart(void);
void IOSPYDaemonHandleTouchPayload(NSData *payload);
void IOSPYDaemonHandleSystemActionPayload(NSData *payload);

#ifdef __cplusplus
}
#endif
