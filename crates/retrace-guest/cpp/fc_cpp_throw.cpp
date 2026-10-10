// M49 first-contact corpus (spec §3b): throw and catch. The unwinder walks the binary's own unwind
// tables and libc++abi's personality routine, all in guest memory.
#include <iostream>
#include <stdexcept>

static int risky(int n) {
    if (n > 1) throw std::runtime_error("boom at " + std::to_string(n));
    return n;
}

int main() {
    try {
        risky(3);
        std::cout << "not thrown" << std::endl;
    } catch (const std::exception &e) {
        std::cout << "caught: " << e.what() << std::endl;
    }
    return 0;
}
