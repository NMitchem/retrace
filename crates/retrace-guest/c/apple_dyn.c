#include <stdio.h>
#include <string.h>
int main(int argc, char **argv, char **envp, char **apple) {
  printf("argv0=%s\n", argv[0]);
  for (int i = 0; apple[i]; i++)
    if (!strncmp(apple[i], "executable_path=", 16)) printf("%s\n", apple[i]);
  return 0;
}
