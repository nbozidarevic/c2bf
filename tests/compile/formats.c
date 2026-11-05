#include <stdio.h>

int main() {
  int a = 0, b = 0, c = 0, d = 0, e = 0, f = 0, g = 0;
  char buf[6];
  scanf("%x %o %i %i %i %x %u", &a, &b, &c, &d, &e, &f, &g);
  printf("%d,%d,%d,%d,%d,%d,%d\n", a, b, c, d, e, f, g);
  scanf("%*d %d %*s %s", &a, buf);
  printf("%d,%s\n", a, buf);
  printf("%o,%o,%X,%X,%ld\n", 255, 8, 171, 10, 42);
  return 0;
}
