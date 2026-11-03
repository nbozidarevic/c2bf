#include <stdio.h>

int add(int a, int b) { return a + b; }
int dbl(int x) { return x * 2; }
int quad(int x) {
  int y = dbl(x);
  return dbl(y);
}
int first(int n) {
  int i = 0;
  while (i < 10) {
    if (i == n) {
      return i * 2;
    }
    i++;
  }
  return 99;
}
int pick(int s) {
  switch (s) {
    case 1:
      return 10;
    case 2:
      return 20;
    default:
      return 30;
  }
}
int count_to(int n) {
  int s = 0;
  int i = 0;
  for (i = 0; i < 5; i++) {
    if (i == n) {
      return s;
    }
    s = s + 10;
  }
  return s;
}
int bump(int *p) {
  *p = *p + 1;
  return *p;
}
int sum(int a[], int n) {
  int s = 0;
  int i = 0;
  while (i < n) {
    s = s + a[i];
    i++;
  }
  return s;
}
void greet() {
  printf("hi ");
  return;
}

int main() {
  printf("%d,%d,%d,%d,", add(20, 22), dbl(21), quad(3), first(3));
  printf("%d,", first(20));
  printf("%d,%d,%d,", pick(1), pick(2), pick(9));
  printf("%d,%d,", count_to(2), count_to(9));
  int x = 5;
  int b = bump(&x);
  printf("%d,%d,", b, x);
  int arr[4];
  arr[0] = 1;
  arr[1] = 2;
  arr[2] = 3;
  arr[3] = 4;
  printf("%d,", sum(arr, 4));
  greet();
  printf("%d\n", add(add(1, 2), add(3, 4)));
  return 0;
}
