#include <stdio.h>

int main() {
  printf("%%\n");
  printf("%u\n", 250);
  printf("%u\n", 0);
  printf("%x\n", 255);
  printf("%x\n", 0);
  printf("%x\n", 16);
  printf("%x\n", 171);
  printf("%d %c %% %u %x\n", 42, 'A', 200, 200);
  return 0;
}
