// M49 first-contact corpus (spec §3b), row S: std::this_thread::sleep_for(50 ms), which libc++
// implements with nanosleep(&ts, &rem): clock_get_time (mach_msg2 1000), then __semwait_signal
// (334) (spec §2e). Prints whether steady_clock (mach_absolute_time) advanced by at least 50 ms.
#include <chrono>
#include <iostream>
#include <thread>

int main() {
    std::cout << "sleeping 50 ms" << std::endl;
    auto t0 = std::chrono::steady_clock::now();
    std::this_thread::sleep_for(std::chrono::milliseconds(50));
    auto dt = std::chrono::steady_clock::now() - t0;
    std::cout << "ok=" << (dt >= std::chrono::milliseconds(50)) << std::endl;
    return 0;
}
