#include <stdio.h>
int main() {
  int x = 0;
  scanf("%d", &x);
  if (x == 0) {
    goto done;
  }
  do {
    x = x + 1;
  } while (x < 3);
  done: printf("%d", x);
}
