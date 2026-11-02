#include <stdio.h>

unsigned int g = 200;
const char h = 'Z';

int main() {
  unsigned int a = 250;
  const int b = 5;
  long c = 3;
  signed char d = 65;
  long long e = 2;
  int sum = a + b + c + d + e;
  printf("%d %d %c\n", sum % 256, g, h);
  for (unsigned int i = 0; i < 3; i++) {
    printf("%d\n", i);
  }
  return 0;
}
