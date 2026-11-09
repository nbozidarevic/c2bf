#include <stdio.h>
int main() {
  int x = 0;
  scanf("%d", &x);
  if (x != 0) {
    do {
      x = x + 1;
    } while (x < 3);
  }
  printf("%d", x);
}
