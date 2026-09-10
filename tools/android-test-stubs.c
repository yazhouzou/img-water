/* Android 测试链接桩：仅用于 cargo test --no-run 的链接检查，永远不会执行。
 * ort 静态库按更高 API level 编译，引用了 API21 libc 没有的 bionic 符号；
 * cdylib(APK) 默认允许未定义符号，测试可执行文件必须全定义，故提供空桩。 */
#include <stddef.h>
#include <sys/types.h>

char *__gnu_strerror_r(int errnum, char *buf, size_t buflen) {
    (void)errnum; (void)buf; (void)buflen;
    return 0;
}

ssize_t __write_chk(int fd, const void *buf, size_t count, size_t bufsize) {
    (void)fd; (void)buf; (void)count; (void)bufsize;
    return 0;
}

void *stderr(void) { return 0; }
