#include <stdio.h>

char g[] = "G!";

int main() {
  char s[6] = "hello";
  char t[] = "hi";
  char *p = "bye";
  char s2[3] = "hi";
  printf("[%s|%s|%s|%s]\n", s, t, p, g);
  printf("[%s]\n", &s[1]);
  s[0] = 'H';
  printf("[%s]\n", s);
  printf("[%s]\n", s2);
  printf("%d,%d,%d\n", sizeof(s), sizeof(t), sizeof("bye"));
  char e[] = "";
  printf("[%s]\n", e);
  char *q = "a\tb";
  printf("[%s]\n", q);
  char *r = "x";
  printf("%d\n", *r);
  printf("[%s]\n", "lit");
  return 0;
}
