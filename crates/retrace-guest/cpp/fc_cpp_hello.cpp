// M49 first-contact corpus (spec §3b): iostream hello, the first C++ program. libc++'s
// std::cout reaches the kernel through the same write path as stdio.
#include <iostream>
#include <string>

int main() {
    std::string who = "retrace";
    std::cout << "hello, " << who << ' ' << 42 << std::endl;
    return 0;
}
