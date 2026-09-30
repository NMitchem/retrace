// Throwaway: the smallest guest that reaches tzload (localtime_r -> tzsetwall_basic).
#include <stdio.h>
#include <time.h>
int main(void) {
    time_t t = 1700000000; struct tm tm;
    localtime_r(&t, &tm);
    printf("%04d-%02d-%02d %02d:%02d %s\n", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_zone);
    return 0;
}
