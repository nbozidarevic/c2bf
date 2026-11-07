#include <stdio.h>

typedef int Row[3];

int get(int m[][3], int i, int j) {
  return m[i][j];
}

int sumrow(int m[2][3], int r) {
  return m[r][0] + m[r][1] + m[r][2];
}

int main() {
  int a[2][3] = {{1, 2, 3}, {4, 5, 6}};
  int b[2][3] = {7, 8, 9, 10};
  int c[][2] = {{1, 2}, {3, 4}, {5, 6}};
  int d[2][3] = {{1}, {4, 5}};
  Row g[2] = {{9, 8, 7}, {6, 5, 4}};
  char w[2][3] = {{'a', 'b', 'c'}, {'d', 'e', 'f'}};
  int *p = 0;
  int *q = 0;
  int i = 0;
  int j = 0;
  a[0][1] = 20;
  a[1][0] += 10;
  printf("%d,%d,%d,%d\n", a[0][0], a[0][1], a[1][0], a[1][2]);
  printf("%d,%d,%d\n", b[0][2], b[1][0], b[1][2]);
  printf("%d,%d,%d\n", c[2][1], d[0][2], d[1][1]);
  printf("%d,%d,%d\n", g[0][2], g[1][0], get(a, 1, 2));
  printf("%d,%d\n", sumrow(a, 0), sumrow(g, 1));
  printf("%d,%d,%d\n", sizeof(w), sizeof(w[0]), sizeof(w[0][0]));
  printf("%c,%c\n", w[0][1], w[1][0]);
  p = a[1];
  printf("%d,%d\n", p[0], p[2]);
  p[1] = 99;
  printf("%d\n", a[1][1]);
  q = *a;
  printf("%d,%d\n", *q, q[2]);
  i = 1;
  j = 1;
  printf("%d,%d\n", a[i][j], a[j][i]);
  a[i][j] = 77;
  printf("%d\n", a[1][1]);
  scanf("%d", &a[0][2]);
  printf("%d\n", a[0][2]);
  scanf("%d", &a[i][j]);
  printf("%d\n", a[1][1]);
  scanf("%d", &g[0][0]);
  printf("%d\n", g[0][0]);
  return 0;
}
