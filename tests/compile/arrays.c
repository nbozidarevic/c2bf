#include <stdio.h>

char g[3];

int main() {
  char a[5];
  int i = 0;
  a[0] = 65;
  a[1] = 66;
  a[2] = a[0] + 5;
  a[3] = a[1] - 1;
  a[4] = 33;
  a[1]++;
  a[2] += 3;
  a[3]--;
  a[(4)] = 63;
  if (a[0] == 65) {
    printf("%c%c%d %d %c\n", a[0], a[1], a[2], a[3], a[4]);
  }
  printf("%d %d %d\n", sizeof(a), sizeof(g), sizeof(a[0]));
  printf("%d\n", a[0] * 2);
  g[0] = 72;
  g[1] = 105;
  g[2] = 0;
  printf("%c%c\n", g[0], g[1]);
  i = i + 1;
  printf("%d\n", i);
  return 0;
}
