#include <stdio.h>

typedef int (*cb)(int);
typedef char *(*scb)(int);
typedef void (*vcb)(int *);

int dbl(int x) { return x * 2; }
int tri(int x) { return x * 3; }
char *greet(int x) { return "hi"; }
void inc(int *p) { *p = *p + 1; }
int apply(cb f, int x) { return f(x); }

int main() {
  cb fp = dbl;
  printf("%d,", fp(4));
  fp = tri;
  printf("%d,", (*fp)(4));
  printf("%d,", apply(fp, 5));
  scb gp = greet;
  printf("%s,", gp(0));
  vcb vp = inc;
  int x = 41;
  vp(&x);
  printf("%d,", x);
  cb fp2 = 0;
  printf("%d,", fp2 == 0);
  fp2 = dbl;
  printf("%d,", fp2(3));
  printf("%d", fp == tri);
  return 0;
}
