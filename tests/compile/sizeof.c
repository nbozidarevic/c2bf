#include <stdio.h>

int main() {
  char c = 'A';
  printf("%d\n", sizeof(char));
  printf("%d\n", sizeof c);
  printf("%d\n", sizeof(c));
  printf("%d\n", sizeof(char) + sizeof(c));
  if (sizeof(char) == 1) {
    printf("one\n");
  }
  return 0;
}
