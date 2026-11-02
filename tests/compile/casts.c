#include <stdio.h>

int main() {
  printf("%d\n", (char)300);
  printf("%d\n", (int)5);
  printf("%d\n", (int)(200 + 55));
  printf("%d\n", (char)'A');
  printf("%d\n", (unsigned int)7);
  int a = (int)200;
  printf("%d\n", (int)a + (char)10);
  printf("%d\n", (int)(char)250);
  return 0;
}
