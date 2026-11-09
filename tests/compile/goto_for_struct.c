#include <stdio.h>
int main() {
  int i = 0;
  int s = 0;
  for (i = 0; i < 10; i = i + 1) {
    if (i == 4) {
      break;
    }
    s = s + i;
  }
  printf("%d,%d", i, s);
}
