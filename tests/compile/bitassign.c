#include <stdio.h>

int main() {
  int a = 12;
  a &= 10;
  printf("%d\n", a);
  a |= 3;
  printf("%d\n", a);
  a ^= 5;
  printf("%d\n", a);
  a <<= 1;
  printf("%d\n", a);
  a >>= 2;
  printf("%d\n", a);
  int b = 200;
  b >>= 3;
  printf("%d\n", b);
  b <<= 2;
  printf("%d\n", b);
  int c = 0x55;
  c ^= 0xFF;
  printf("%d\n", c);
  return 0;
}
