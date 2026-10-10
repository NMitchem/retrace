// M49 first-contact corpus (spec §3b): std::thread and std::mutex, joined. Two threads add to one
// counter under the mutex. retrace's cooperative scheduler runs each worker to completion while
// main is blocked in join, so the mutex is never contended under retrace; fc_mutex is the row that
// contends.
#include <iostream>
#include <mutex>
#include <thread>

int main() {
    std::mutex m;
    long counter = 0;
    auto work = [&](long n) {
        for (long i = 0; i < n; i++) {
            std::lock_guard<std::mutex> g(m);
            counter += 1;
        }
    };
    std::thread a(work, 1000), b(work, 2000);
    a.join();
    b.join();
    std::cout << "counter=" << counter << std::endl;
    return 0;
}
