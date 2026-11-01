// M54 t3. An x86_64-only image (built -arch x86_64): natively it runs under Rosetta and prints x86; retrace
// refuses an execve of it by name, since it runs arm64 only.
#include <stdio.h>
int main(void) { puts("x86"); return 0; }
