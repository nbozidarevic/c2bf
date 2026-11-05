#include <stdio.h>

char buf[3] = "hi";

char *get(void);
char *get(void) {
  return buf;
}

char *other() {
  return &buf[0];
}

int main() {
  char *p = get();
  printf("%s %s %d\n", p, other(), *get());
  return 0;
}
