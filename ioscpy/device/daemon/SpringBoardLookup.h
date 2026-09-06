#import <Foundation/Foundation.h>
#import <mach/mach.h>

#ifdef __cplusplus
extern "C" {
#endif

pid_t IOSPYSpringBoardPid(void);
BOOL IOSPYSpringBoardTask(task_t *outTask);
uint64_t IOSPYRemoteSymbol(task_t task, const char *imageHint, const char *symbol);
BOOL IOSPYSpringBoardRenderClient(uint32_t *outClient);
void IOSPYDyldImageStats(task_t task, uint32_t *outVersion, uint32_t *outCount);
NSString *IOSPYDyldSamplePaths(task_t task);

#ifdef __cplusplus
}
#endif
