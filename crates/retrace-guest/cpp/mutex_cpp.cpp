// M52 §3f fixture: C++ std::mutex under contention (libc++ wraps pthread_mutex_t). Main holds the
// mutex across a join of a busy worker while four std::threads add to one counter, so each finds it
// held and goes to the kernel as psynch_mutexwait (301).
#include <cstdio>
#include <mutex>
#include <thread>
#include <vector>

int main() {
    std::mutex m;
    int counter = 0;
    std::vector<std::thread> cs;
    m.lock();
    for (int i = 0; i < 4; i++)
        cs.emplace_back([&] {
            for (int k = 0; k < 25; k++) {
                std::lock_guard<std::mutex> g(m);
                counter++;
            }
        });
    std::thread w([] { volatile long s = 0; for (long i = 0; i < 100000; i++) s += i; });
    w.join();
    m.unlock();
    for (auto &t : cs) t.join();
    std::printf("counter=%d\n", counter);
    return 0;
}
