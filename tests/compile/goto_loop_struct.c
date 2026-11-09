#include <stdio.h>
int main() {
  int i = 0;
  int s = 0;
  while (i < 10) {
    i = i + 1;
    if (i == 7) {
      break;
    }
    if (i % 2 == 0) {
      s = s + 10;
    } else {
      s = s + 1;
      s = s + 10;
    }
  }
  printf("%d,%d", i, s);
}
