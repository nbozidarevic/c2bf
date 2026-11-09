#include <stdio.h>
int main() {
  int x = 1;
  if (0) {
    x = 2;
    printf("%d,", x);
  }
  printf("%d", x);
}
