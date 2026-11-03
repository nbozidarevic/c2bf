#include <stdio.h>

int main() {
  char s[8] = "";
  char t[4] = "";
  char *p = 0;
  char a[8] = "";
  char u[6] = "keep";
  int n = 0;
  scanf("%s", s);
  printf("[%s]\n", s);
  scanf("%s", t);
  printf("[%s]\n", t);
  p = a;
  scanf("%s", p);
  printf("[%s]\n", a);
  scanf("%s", &a[2]);
  printf("[%s|%s]\n", a, &a[2]);
  scanf("%s %d", s, &n);
  printf("[%s]%d\n", s, n);
  scanf("%s", u);
  printf("[%s]\n", u);
  return 0;
}
