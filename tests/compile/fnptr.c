#include <stdio.h>

int dbl(int x) { return x * 2; }
int tri(int x) { return x * 3; }
int add(int a, int b) { return a + b; }
int *getp(int *p) { return p; }
char *greet(void) { return "hi"; }
void inc(int *p) { *p = *p + 1; }
int apply(int (*cb)(int), int x) { return cb(x) + (*cb)(x); }
int pick(int c, int (*a)(int), int (*b)(int)) {
  if (c) {
    return a(10);
  }
  return b(10);
}

int main() {
  int (*fp)(int) = dbl;
  int (*fp2)(int, int) = add;
  int *(*pp)(int *) = getp;
  char *(*gp)(void) = greet;
  void (*vp)(int *) = inc;
  int x = 0;
  printf("%d,", fp(4));
  printf("%d,", (*fp)(4));
  printf("%d,", (**fp)(4));
  fp = tri;
  printf("%d,", fp(4));
  printf("%d,", apply(fp, 5));
  printf("%d,", fp2(3, 4));
  int v = 21;
  printf("%d,", *pp(&v));
  printf("%s,", gp());
  vp(&x);
  printf("%d,", x);
  printf("%d,", pick(1, dbl, tri));
  printf("%d,", pick(0, dbl, tri));
  fp = 0;
  printf("%d,", fp == 0);
  printf("%d,", fp != tri);
  fp = &dbl;
  printf("%d,", fp(1));
  int (*fp3)(int) = 0;
  printf("%d,", fp3 == 0);
  printf("%d,", apply(tri, 1) + apply(dbl, 2));
  int (*fp4)(int) = fp;
  printf("%d,", fp4(6));
  int (*fp5)(int) = 1 ? dbl : tri;
  printf("%d,", fp5(2));
  printf("%d\n", *getp(&x));
  return 0;
}
