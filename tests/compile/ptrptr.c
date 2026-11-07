#include <stdio.h>

typedef int *P;
typedef int **PP;

int **getpp(int **q) {
  return q;
}

int sum2(int **pp) {
  return **pp + *pp[0];
}

int main() {
  int x = 0;
  int y = 0;
  int *p = 0;
  int **pp = 0;
  int a[4] = {10, 20, 30, 40};
  int *q = 0;
  int **qp = 0;
  char s[4] = "hi";
  char *sp = 0;
  char **spp = 0;
  int i = 0;
  p = &x;
  pp = &p;
  **pp = 65;
  printf("%d,%d\n", x, **pp);
  **pp += 2;
  printf("%d\n", x);
  (**pp)++;
  printf("%d\n", x);
  **pp = **pp * 2;
  printf("%d\n", x);
  q = a;
  qp = &q;
  printf("%d,%d,%d\n", (*qp)[1], qp[0][2], **qp);
  i = 2;
  printf("%d,%d\n", (*qp)[i], qp[i - 2][3]);
  qp[0] = &a[3];
  printf("%d\n", *q);
  *pp = &y;
  **pp = 7;
  printf("%d,%d\n", x, y);
  pp = getpp(&p);
  printf("%d\n", **pp);
  printf("%d\n", sum2(pp));
  printf("%d\n", **getpp(pp));
  P *ap = &p;
  PP tp = &p;
  printf("%d,%d\n", **ap, **tp);
  sp = s;
  spp = &sp;
  printf("%s,%c\n", *spp, **spp);
  **spp = 'A';
  printf("%s\n", s);
  scanf("%d", *pp);
  printf("%d\n", x);
  scanf("%d", pp[0]);
  printf("%d\n", x);
  return 0;
}
