#import "Inject.h"
#import "Paths.h"
#import "SpringBoardLookup.h"

#import <dlfcn.h>
#import <mach/mach.h>
#import <mach/thread_act.h>
#import <mach/thread_status.h>
#import <stdint.h>
#import <string.h>
#import <unistd.h>

static NSString *gInjectError = nil;

NSString *IOSPYInjectLastError(void) {
    return gInjectError ?: @"";
}

static void setInjectError(NSString *msg) {
    gInjectError = msg;
    NSLog(@"[ioscpyd] inject: %@", msg);
}

static void jailbreakTrustAndDebug(pid_t pid, const char *dylib) {
    const char *libs[] = {
        "/var/jb/usr/lib/libjailbreak.dylib",
        "/usr/lib/libjailbreak.dylib",
        NULL,
    };
    for (const char **p = libs; *p; p++) {
        void *h = dlopen(*p, RTLD_NOW);
        if (!h) {
            continue;
        }
        int (*trust)(const char *) = (int (*)(const char *))dlsym(h, "jbclient_trust_file_by_path");
        int (*debugPid)(uint64_t, int) =
            (int (*)(uint64_t, int))dlsym(h, "jbclient_platform_set_process_debugged");
        if (trust) {
            trust(dylib);
        }
        if (debugPid) {
            debugPid((uint64_t)pid, 1);
        }
        NSLog(@"[ioscpyd] inject: libjailbreak from %s trust=%d debug=%d", *p, trust != NULL,
              debugPid != NULL);
        return;
    }
}

static BOOL injectDlopen(pid_t pid, const char *dylib) {
    jailbreakTrustAndDebug(pid, dylib);
    task_t task = MACH_PORT_NULL;
    kern_return_t kr = task_for_pid(mach_task_self(), pid, &task);
    if (kr != KERN_SUCCESS || !MACH_PORT_VALID(task)) {
        setInjectError([NSString stringWithFormat:@"task_for_pid(%d) failed: %s", pid,
                                                 mach_error_string(kr)]);
        return NO;
    }

    uint64_t dlopenRemote = IOSPYRemoteSymbol(task, "libdyld", "_dlopen");
    if (!dlopenRemote) {
        dlopenRemote = IOSPYRemoteSymbol(task, "libdyld.dylib", "_dlopen");
    }
    if (!dlopenRemote) {
        dlopenRemote = IOSPYRemoteSymbol(task, "libSystem", "_dlopen");
    }
    if (!dlopenRemote) {
        dlopenRemote = IOSPYRemoteSymbol(task, NULL, "_dlopen");
    }
    uint64_t pthreadExitRemote = IOSPYRemoteSymbol(task, "libsystem_pthread", "_pthread_exit");
    if (!pthreadExitRemote) {
        pthreadExitRemote = IOSPYRemoteSymbol(task, "libSystem", "_pthread_exit");
    }
    if (!dlopenRemote) {
        setInjectError(@"remote _dlopen not found (dyld image list empty or no exports)");
        mach_port_deallocate(mach_task_self(), task);
        return NO;
    }

    vm_size_t pathLen = strlen(dylib) + 1;
    vm_address_t remotePath = 0;
    kr = vm_allocate(task, &remotePath, pathLen, VM_FLAGS_ANYWHERE);
    if (kr != KERN_SUCCESS) {
        setInjectError(@"vm_allocate path failed");
        mach_port_deallocate(mach_task_self(), task);
        return NO;
    }
    kr = vm_write(task, remotePath, (vm_offset_t)(uintptr_t)dylib, (mach_msg_type_number_t)pathLen);
    if (kr != KERN_SUCCESS) {
        vm_deallocate(task, remotePath, pathLen);
        mach_port_deallocate(mach_task_self(), task);
        return NO;
    }

    vm_address_t remoteStack = 0;
    const vm_size_t stackSize = 0x4000;
    kr = vm_allocate(task, &remoteStack, stackSize, VM_FLAGS_ANYWHERE);
    if (kr != KERN_SUCCESS) {
        vm_deallocate(task, remotePath, pathLen);
        mach_port_deallocate(mach_task_self(), task);
        return NO;
    }
    vm_protect(task, remoteStack, stackSize, FALSE, VM_PROT_READ | VM_PROT_WRITE);

    arm_thread_state64_t st;
    memset(&st, 0, sizeof(st));
    vm_address_t sp = (remoteStack + stackSize - 0x20) & ~(vm_address_t)0xF;
    uint64_t lr = pthreadExitRemote ? pthreadExitRemote : dlopenRemote;
#if defined(__darwin_arm_thread_state64_set_pc_fptr)
    // Xcode opaque arm64e thread state (no direct __pc / __lr / __sp).
    __darwin_arm_thread_state64_set_pc_fptr(st, (void *)(uintptr_t)dlopenRemote);
    __darwin_arm_thread_state64_set_lr_fptr(st, (void *)(uintptr_t)lr);
    __darwin_arm_thread_state64_set_sp(st, sp);
#if defined(__darwin_arm_thread_state64_set_x)
    __darwin_arm_thread_state64_set_x(st, 0, remotePath);
    __darwin_arm_thread_state64_set_x(st, 1, (uint64_t)RTLD_NOW);
#else
    st.__x[0] = remotePath;
    st.__x[1] = RTLD_NOW;
#endif
#else
    st.__pc = dlopenRemote;
    st.__lr = lr;
    st.__sp = sp;
    st.__x[0] = remotePath;
    st.__x[1] = RTLD_NOW;
#endif

    thread_act_t thread = MACH_PORT_NULL;
    kr = thread_create_running(task, ARM_THREAD_STATE64, (thread_state_t)&st, ARM_THREAD_STATE64_COUNT,
                               &thread);
    if (kr != KERN_SUCCESS) {
        kr = thread_create(task, &thread);
        if (kr == KERN_SUCCESS) {
            kr = thread_set_state(thread, ARM_THREAD_STATE64, (thread_state_t)&st,
                                  ARM_THREAD_STATE64_COUNT);
            if (kr == KERN_SUCCESS) {
                kr = thread_resume(thread);
            }
        }
    }
    if (kr != KERN_SUCCESS) {
        setInjectError([NSString stringWithFormat:@"thread start: %s (remote dlopen=0x%llx)",
                                                 mach_error_string(kr), dlopenRemote]);
        vm_deallocate(task, remotePath, pathLen);
        vm_deallocate(task, remoteStack, stackSize);
        if (MACH_PORT_VALID(thread)) {
            mach_port_deallocate(mach_task_self(), thread);
        }
        mach_port_deallocate(mach_task_self(), task);
        return NO;
    }
    setInjectError([NSString stringWithFormat:@"dlopen-thread ok pc=0x%llx (arm64 dylib may still fail)",
                                              dlopenRemote]);
    if (MACH_PORT_VALID(thread)) {
        mach_port_deallocate(mach_task_self(), thread);
    }
    mach_port_deallocate(mach_task_self(), task);
    return YES;
}

void IOSPYInjectHookIfNeeded(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        NSString *dylib = IOSPYPath(@"/usr/lib/TweakInject/ioscpyhook.dylib");
        if (![[NSFileManager defaultManager] fileExistsAtPath:dylib]) {
            NSString *alt = IOSPYPath(@"/Library/MobileSubstrate/DynamicLibraries/ioscpyhook.dylib");
            if ([[NSFileManager defaultManager] fileExistsAtPath:alt]) {
                dylib = alt;
            } else {
                setInjectError(@"ioscpyhook.dylib missing");
                return;
            }
        }
        pid_t pid = IOSPYSpringBoardPid();
        if (pid <= 0) {
            setInjectError(@"SpringBoard pid not found");
            return;
        }
        NSLog(@"[ioscpyd] inject: SpringBoard pid=%d dylib=%@", pid, dylib);
        injectDlopen(pid, dylib.fileSystemRepresentation);
    });
}
