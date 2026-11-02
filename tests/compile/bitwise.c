#include <stdio.h>

int main() {
  printf("%d\n", 12 & 10);
  printf("%d\n", 12 | 10);
  printf("%d\n", 12 ^ 10);
  printf("%d\n", 0x55 & 0xAA);
  printf("%d\n", 0x55 | 0xAA);
  printf("%d\n", 0x55 ^ 0xAA);
  printf("%d\n", ~0 & 0xFF);
  printf("%d\n", 1 << 7);
  printf("%d\n", 3 << 2);
  printf("%d\n", (128 << 1) & 0xFF);
  printf("%d\n", 200 >> 3);
  printf("%d\n", 255 >> 8);
  printf("%d\n", (1 << 8) & 0xFF);
  printf("%d\n", 5 << 0);
  printf("%d\n", 5 >> 0);
  printf("%d\n", 1 + 2 << 1);
  printf("%d\n", 8 >> 1 + 1);
  printf("%d\n", 2 << 1 << 1);
  printf("%d\n", 12 & 10 == 8);
  printf("%d\n", 1 | 2 & 3);
  printf("%d\n", 10 ^ 3 & 5);
  int x = 5;
  printf("%d %d\n", (x++) & 3, x);
  x = 1;
  printf("%d %d\n", (x++) << (x++), x);
  x = 2;
  printf("%d %d\n", (x++) >> (x++), x);
  int y = 0;
  printf("%d %d %d\n", (x = 2) << (y = 3), x, y);
  return 0;
}
