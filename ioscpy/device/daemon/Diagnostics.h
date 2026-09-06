#import <Foundation/Foundation.h>

#ifdef __cplusplus
extern "C" {
#endif

void IOSPYDiagReset(void);
void IOSPYDiag(NSString *line);
void IOSPYDiagf(NSString *fmt, ...) NS_FORMAT_FUNCTION(1, 2);
NSArray<NSString *> *IOSPYDiagLines(void);
NSString *IOSPYDiagSummary(void);

// Full pass: SpringBoard, inject, capture, tweak channel. Safe to call often.
void IOSPYDiagRunSession(void);

#ifdef __cplusplus
}
#endif
