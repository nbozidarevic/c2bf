#include <stdio.h>

char g[4] = {1, 2};
char h[] = {3, 4, 5};

int sum2(int q) {
  int a[2] = {q, q + 1};
  return a[0] + a[1];
}

int main() {
  int x = {7};
  int a[3] = {x, x + 1, 0};
  char t[] = {'a', 'b'};
  char s[] = {"hi"};
  printf("%d,%d,%d,%d,%d,%d,%d,%d,%s,%d,%d,%d\n", g[0], g[1], g[2], g[3],
         h[0], h[2], x, a[1], s, t[0], t[1], sum2(10));
  return 0;
}
