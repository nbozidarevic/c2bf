#include <stdio.h>
int main() {
  int x = 1;
  goto done;
  x = 2;
  printf("%d,", x);
  done: printf("%d", x);
}
