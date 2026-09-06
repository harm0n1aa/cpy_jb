#import "SpringBoardLookup.h"

#import <mach/mach.h>
#import <mach/task_info.h>
#import <mach-o/loader.h>
#import <mach-o/nlist.h>
#import <stdint.h>
#import <stdlib.h>
#import <string.h>
#import <sys/sysctl.h>
#import <unistd.h>
#import <errno.h>

#ifndef PROC_ALL_PIDS
#define PROC_ALL_PIDS 1
#endif
#ifndef PROC_PIDT_SHORTBSDINFO
#define PROC_PIDT_SHORTBSDINFO 13
#endif

#ifdef __cplusplus
extern "C" {
#endif
int proc_listpids(uint32_t type, uint32_t typeinfo, void *buffer, int buffersize);
int proc_pidpath(int pid, void *buffer, uint32_t buffersize);
int proc_name(int pid, void *buffer, uint32_t buffersize);
int proc_pidinfo(int pid, int flavor, uint64_t arg, void *buffer, int buffersize);
extern mach_port_t bootstrap_port;
kern_return_t bootstrap_look_up(mach_port_t bp, const char *service_name, mach_port_t *sp);
#ifdef __cplusplus
}
#endif

struct iospy_bsdshortinfo {
    uint32_t pbsi_pid;
    uint32_t pbsi_ppid;
    uint32_t pbsi_pgid;
    uint32_t pbsi_status;
    char pbsi_comm[16];
    uint32_t pbsi_flags;
    int32_t pbsi_uid;
    int32_t pbsi_gid;
    int32_t pbsi_ruid;
    int32_t pbsi_rgid;
    int32_t pbsi_svuid;
    int32_t pbsi_svgid;
    uint32_t pbsi_rfu;
};

static BOOL rd(task_t task, uint64_t addr, void *buf, size_t len) {
    vm_size_t got = len;
    kern_return_t kr =
        vm_read_overwrite(task, (vm_address_t)addr, (vm_size_t)len, (vm_address_t)buf, &got);
    return kr == KERN_SUCCESS && got == len;
}

static BOOL commIsSpringBoard(const char *comm) {
    return comm && strcmp(comm, "SpringBoard") == 0;
}

pid_t IOSPYSpringBoardPid(void) {
    enum { kBuf = 64 * 1024 };
    pid_t *pids = (pid_t *)malloc(kBuf);
    if (pids) {
        int bytes = proc_listpids(PROC_ALL_PIDS, 0, pids, kBuf);
        int n = bytes > 0 ? bytes / (int)sizeof(pid_t) : 0;
        char name[32];
        for (int i = 0; i < n; i++) {
            if (pids[i] <= 0) {
                continue;
            }
            memset(name, 0, sizeof(name));
            if (proc_name(pids[i], name, sizeof(name) - 1) > 0 && commIsSpringBoard(name)) {
                pid_t found = pids[i];
                free(pids);
                return found;
            }
            struct iospy_bsdshortinfo info;
            memset(&info, 0, sizeof(info));
            if (proc_pidinfo(pids[i], PROC_PIDT_SHORTBSDINFO, 0, &info, sizeof(info)) > 0 &&
                commIsSpringBoard(info.pbsi_comm)) {
                pid_t found = pids[i];
                free(pids);
                return found;
            }
        }
        free(pids);
    }
    int mib[4] = {CTL_KERN, KERN_PROC, KERN_PROC_ALL, 0};
    size_t size = 0;
    if (sysctl(mib, 4, NULL, &size, NULL, 0) == 0 && size > 0) {
        struct kinfo_proc *list = (struct kinfo_proc *)malloc(size);
        if (list && sysctl(mib, 4, list, &size, NULL, 0) == 0) {
            int n = (int)(size / sizeof(*list));
            for (int i = 0; i < n; i++) {
                if (commIsSpringBoard(list[i].kp_proc.p_comm)) {
                    pid_t found = list[i].kp_proc.p_pid;
                    free(list);
                    return found;
                }
            }
        }
        free(list);
    }
    return 0;
}

BOOL IOSPYSpringBoardTask(task_t *outTask) {
    if (!outTask) {
        return NO;
    }
    pid_t pid = IOSPYSpringBoardPid();
    if (pid <= 0) {
        return NO;
    }
    task_t task = MACH_PORT_NULL;
    kern_return_t kr = task_for_pid(mach_task_self(), pid, &task);
    if (kr != KERN_SUCCESS || !MACH_PORT_VALID(task)) {
        return NO;
    }
    *outTask = task;
    return YES;
}

static uint64_t readUleb(const uint8_t **p, const uint8_t *end) {
    uint64_t r = 0;
    int s = 0;
    while (*p < end) {
        uint8_t b = *(*p)++;
        r |= (uint64_t)(b & 0x7f) << s;
        if (!(b & 0x80)) {
            break;
        }
        s += 7;
    }
    return r;
}

static uint64_t walkExportTrie(const uint8_t *start, size_t size, const uint8_t *node, const char *need,
                               uint64_t slide) {
    const uint8_t *end = start + size;
    if (node < start || node >= end) {
        return 0;
    }
    const uint8_t *p = node;
    uint64_t terminal = readUleb(&p, end);
    const uint8_t *children = p + terminal;
    if (terminal && need[0] == 0) {
        uint64_t flags = readUleb(&p, end);
        if ((flags & 0x03) == 0) {
            uint64_t addr = readUleb(&p, end);
            return slide + addr;
        }
    }
    if (children >= end) {
        return 0;
    }
    p = children;
    uint8_t nchild = *p++;
    for (uint8_t i = 0; i < nchild && p < end; i++) {
        const char *edge = (const char *)p;
        size_t elen = strnlen(edge, (size_t)(end - p));
        p += elen + 1;
        uint64_t childOff = readUleb(&p, end);
        size_t nlen = strlen(need);
        if (nlen >= elen && memcmp(need, edge, elen) == 0) {
            uint64_t found = walkExportTrie(start, size, start + childOff, need + elen, slide);
            if (found) {
                return found;
            }
        }
    }
    return 0;
}

static uint64_t symbolInImage(task_t task, uint64_t mhAddr, const char *symbol) {
    struct mach_header_64 mh;
    if (!rd(task, mhAddr, &mh, sizeof(mh)) || mh.magic != MH_MAGIC_64) {
        return 0;
    }
    uint64_t slide = 0;
    uint64_t linkeditVm = 0, linkeditFile = 0;
    struct symtab_command sy;
    memset(&sy, 0, sizeof(sy));
    BOOL haveSy = NO;
    uint32_t exportOff = 0, exportSize = 0;
    BOOL haveExport = NO;
    uint64_t cursor = mhAddr + sizeof(struct mach_header_64);
    for (uint32_t i = 0; i < mh.ncmds; i++) {
        struct load_command lc;
        if (!rd(task, cursor, &lc, sizeof(lc)) || lc.cmdsize < sizeof(lc)) {
            break;
        }
        if (lc.cmd == LC_SEGMENT_64) {
            struct segment_command_64 seg;
            if (rd(task, cursor, &seg, sizeof(seg))) {
                if (strncmp(seg.segname, "__TEXT", 16) == 0) {
                    slide = mhAddr - seg.vmaddr;
                }
                if (strncmp(seg.segname, "__LINKEDIT", 16) == 0) {
                    linkeditVm = seg.vmaddr;
                    linkeditFile = seg.fileoff;
                }
            }
        } else if (lc.cmd == LC_SYMTAB) {
            if (rd(task, cursor, &sy, sizeof(sy))) {
                haveSy = YES;
            }
        } else if (lc.cmd == LC_DYLD_INFO_ONLY || lc.cmd == LC_DYLD_INFO) {
            struct dyld_info_command dy;
            if (rd(task, cursor, &dy, sizeof(dy))) {
                exportOff = dy.export_off;
                exportSize = dy.export_size;
                haveExport = exportSize > 0;
            }
        } else if (lc.cmd == 0x80000033) { // LC_DYLD_EXPORTS_TRIE
            struct linkedit_data_command led;
            if (rd(task, cursor, &led, sizeof(led))) {
                exportOff = led.dataoff;
                exportSize = led.datasize;
                haveExport = exportSize > 0;
            }
        }
        cursor += lc.cmdsize;
    }
    if (haveSy && linkeditVm && sy.nsyms && sy.nsyms < 200000) {
        uint64_t linkeditBase = slide + linkeditVm - linkeditFile;
        uint64_t symAddr = linkeditBase + sy.symoff;
        uint64_t strAddr = linkeditBase + sy.stroff;
        size_t n = sy.nsyms < 8192 ? sy.nsyms : 8192;
        for (size_t i = 0; i < n; i++) {
            struct nlist_64 nl;
            if (!rd(task, symAddr + i * sizeof(nl), &nl, sizeof(nl))) {
                break;
            }
            if (nl.n_un.n_strx == 0 || (nl.n_type & N_STAB)) {
                continue;
            }
            char name[96];
            memset(name, 0, sizeof(name));
            if (!rd(task, strAddr + nl.n_un.n_strx, name, sizeof(name) - 1)) {
                continue;
            }
            if (strcmp(name, symbol) == 0) {
                return nl.n_value + slide;
            }
        }
    }
    if (haveExport && exportSize > 0 && exportSize < 2 * 1024 * 1024 && linkeditVm) {
        uint64_t linkeditBase = slide + linkeditVm - linkeditFile;
        uint8_t *trie = (uint8_t *)malloc(exportSize);
        if (trie && rd(task, linkeditBase + exportOff, trie, exportSize)) {
            const char *need = symbol[0] == '_' ? symbol + 1 : symbol;
            uint64_t found = walkExportTrie(trie, exportSize, trie, need, slide);
            free(trie);
            if (found) {
                return found;
            }
        } else {
            free(trie);
        }
    }
    return 0;
}

uint64_t IOSPYRemoteSymbol(task_t task, const char *imageHint, const char *symbol) {
    struct task_dyld_info info;
    mach_msg_type_number_t count = TASK_DYLD_INFO_COUNT;
    if (task_info(task, TASK_DYLD_INFO, (task_info_t)&info, &count) != KERN_SUCCESS ||
        info.all_image_info_addr == 0) {
        return 0;
    }
    uint8_t head[32];
    if (!rd(task, info.all_image_info_addr, head, sizeof(head))) {
        return 0;
    }
    uint32_t version = 0, nimg = 0;
    uint64_t array = 0;
    memcpy(&version, head, 4);
    memcpy(&nimg, head + 4, 4);
    memcpy(&array, head + 8, 8);
    if (nimg == 0 || nimg > 2048 || array == 0) {
        return 0;
    }
    const char *syms[] = {symbol, NULL};
    if (symbol[0] == '_') {
        syms[0] = symbol;
        syms[1] = symbol + 1;
    } else {
        static char buf[96];
        snprintf(buf, sizeof(buf), "_%s", symbol);
        syms[0] = buf;
        syms[1] = symbol;
    }
    for (uint32_t i = 0; i < nimg; i++) {
        uint64_t rec[3];
        if (!rd(task, array + (uint64_t)i * 24, rec, sizeof(rec))) {
            break;
        }
        char path[256];
        memset(path, 0, sizeof(path));
        if (rec[1]) {
            rd(task, rec[1], path, sizeof(path) - 1);
        }
        if (imageHint && imageHint[0] && !strstr(path, imageHint)) {
            continue;
        }
        for (const char **s = syms; *s; s++) {
            uint64_t found = symbolInImage(task, rec[0], *s);
            if (found) {
                return found;
            }
        }
    }
    if (imageHint && imageHint[0]) {
        return IOSPYRemoteSymbol(task, NULL, symbol);
    }
    return 0;
}

void IOSPYDyldImageStats(task_t task, uint32_t *outVersion, uint32_t *outCount) {
    if (outVersion) {
        *outVersion = 0;
    }
    if (outCount) {
        *outCount = 0;
    }
    struct task_dyld_info info;
    mach_msg_type_number_t count = TASK_DYLD_INFO_COUNT;
    if (task_info(task, TASK_DYLD_INFO, (task_info_t)&info, &count) != KERN_SUCCESS ||
        info.all_image_info_addr == 0) {
        return;
    }
    uint8_t head[16];
    if (!rd(task, info.all_image_info_addr, head, sizeof(head))) {
        return;
    }
    if (outVersion) {
        memcpy(outVersion, head, 4);
    }
    if (outCount) {
        memcpy(outCount, head + 4, 4);
    }
}

NSString *IOSPYDyldSamplePaths(task_t task) {
    struct task_dyld_info info;
    mach_msg_type_number_t count = TASK_DYLD_INFO_COUNT;
    if (task_info(task, TASK_DYLD_INFO, (task_info_t)&info, &count) != KERN_SUCCESS ||
        info.all_image_info_addr == 0) {
        return @"dyld: task_info failed";
    }
    uint8_t head[16];
    if (!rd(task, info.all_image_info_addr, head, sizeof(head))) {
        return @"dyld: vm_read header failed";
    }
    uint32_t nimg = 0;
    uint64_t array = 0;
    memcpy(&nimg, head + 4, 4);
    memcpy(&array, head + 8, 8);
    if (nimg == 0 || array == 0) {
        return @"dyld: image list empty";
    }
    NSMutableString *s = [NSMutableString stringWithString:@"images: "];
    uint32_t show = nimg < 6 ? nimg : 6;
    for (uint32_t i = 0; i < show; i++) {
        uint64_t rec[3];
        if (!rd(task, array + (uint64_t)i * 24, rec, sizeof(rec))) {
            break;
        }
        char path[128];
        memset(path, 0, sizeof(path));
        if (rec[1]) {
            rd(task, rec[1], path, sizeof(path) - 1);
        }
        const char *base = strrchr(path, '/');
        [s appendFormat:@"%@%@", i ? @", " : @"", base ? @(base + 1) : @(path)];
    }
    if (nimg > show) {
        [s appendFormat:@" …+%u", nimg - show];
    }
    return s;
}

BOOL IOSPYSpringBoardRenderClient(uint32_t *outClient) {
    if (!outClient) {
        return NO;
    }
    task_t task = MACH_PORT_NULL;
    if (!IOSPYSpringBoardTask(&task)) {
        return NO;
    }
    mach_port_t sbBootstrap = MACH_PORT_NULL;
    kern_return_t kr = task_get_special_port(task, TASK_BOOTSTRAP_PORT, &sbBootstrap);
    mach_port_deallocate(mach_task_self(), task);
    if (kr != KERN_SUCCESS || !MACH_PORT_VALID(sbBootstrap)) {
        return NO;
    }
    const char *names[] = {"com.apple.CARenderServer", "com.apple.windowserver.active", NULL};
    BOOL ok = NO;
    for (const char **s = names; *s; s++) {
        mach_port_t port = MACH_PORT_NULL;
        if (bootstrap_look_up(sbBootstrap, *s, &port) == KERN_SUCCESS && MACH_PORT_VALID(port)) {
            *outClient = (uint32_t)port;
            ok = YES;
            break;
        }
    }
    mach_port_deallocate(mach_task_self(), sbBootstrap);
    return ok;
}
