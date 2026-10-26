#include "log.h"

void log_init(void) {}
void log_set_level(LogLevel level) { (void)level; }

void log_printf(LogLevel level, const char* module, const char* format, ...)
{
    if (level == Debug)
        return;
    va_list arguments;
    va_start(arguments, format);
    fprintf(stderr, "%s: ", module);
    vfprintf(stderr, format, arguments);
    fputc('\n', stderr);
    va_end(arguments);
}
