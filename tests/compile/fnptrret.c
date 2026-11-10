#include <stdio.h>

typedef int (*cb)(int);

int dbl(int x) { return x * 2; }
int tri(int x) { return x * 3; }
void inc(int *p) { *p = *p + 1; }

cb choose(int c) {
  if (c) {
    return dbl;
  }
  return tri;
}
int (*make(int c))(int) {
  if (c) {
    return tri;
  }
  return dbl;
}
cb nullish(int c) {
  if (c) {
    return dbl;
  }
  return 0;
}
void (*vmaker(int c))(int *) { return inc; }
int (*get(void))(int) { return tri; }

int main() {
  printf("%d,", choose(1)(4));
  printf("%d,", (*choose(0))(4));
  printf("%d,", make(1)(4));
  printf("%d,", make(0)(4));
  cb fp = get();
  printf("%d,", fp(4));
  printf("%d,", get()(4));
  cb np = nullish(0);
  printf("%d,", np == 0);
  int x = 10;
  vmaker(1)(&x);
  printf("%d", x);
  return 0;
}
