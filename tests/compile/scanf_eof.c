#include <stdio.h>
int main() {
  int a = 0;
  int b = 0;
  int c = 0;
  char s[8] = "";
  scanf("%d %i %o", &a, &b, &c);
  scanf("%s", s);
  printf("%d,%d,%d,%s", a, b, c, s);
}
