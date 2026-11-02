#include <stdio.h>

unsigned int g = 200;
const char h = 'Z';

int main() {
  unsigned int a = 250;
  const int b = 5;
  long c = 3;
  signed char d = 65;
  long long e = 2;
  printf("%d\n", a % 100);
  printf("%d\n", b + c + e);
  printf("%d %c %d\n", d, h, g);
  for (unsigned int i = 0; i < 3; i++) {
    printf("%d\n", i);
  }
  return 0;
}
