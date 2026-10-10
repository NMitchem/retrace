// M49 first-contact corpus (spec §3b): an uncaught exception. std::terminate prints to stderr and
// calls abort(), so natively SIGABRT (134) and the trace's terminal event is `Event::Signal`.
// stdout is flushed before the throw.
#include <iostream>
#include <stdexcept>

int main() {
    std::cout << "throwing without a catch" << std::endl;
    throw std::runtime_error("nobody catches this");
}
