#include <stdio.h>

static int counter = 10;
extern int shared;
int shared = 7;

static int get(void) {
  return counter;
}

int main() {
  register int r = 3;
  auto int a = 4;
  printf("%d,%d,%d,%d\n", counter, shared, get(), r + a);
  return 0;
}
