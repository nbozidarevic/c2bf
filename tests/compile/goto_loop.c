#include <stdio.h>
int main() {
  int i = 0;
  int s = 0;
  while (i < 10) {
    i = i + 1;
    if (i == 7) {
      goto done;
    }
    if (i % 2 == 0) {
      goto even;
    }
    s = s + 1;
    even: s = s + 10;
  }
  done: printf("%d,%d", i, s);
}
