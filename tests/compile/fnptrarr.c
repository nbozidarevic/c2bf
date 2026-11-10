#include <stdio.h>

typedef int (*cb)(int);
typedef int (*cb3[2])(int);

int dbl(int x) { return x * 2; }
int tri(int x) { return x * 3; }
int quad(int x) { return x * 4; }

int callall(cb a[], int x) { return a[0](x) + a[1](x); }
int callarr(int (*a[3])(int), int x) { return a[2](x); }

int main() {
  int (*arr[3])(int) = {dbl, tri, 0};
  cb arr2[2] = {tri, dbl};
  cb3 arr3;
  arr3[0] = quad;
  arr3[1] = dbl;
  int i = 1;
  printf("%d,", arr[0](4));
  printf("%d,", arr[i](4));
  printf("%d,", (*arr[1])(4));
  arr[2] = quad;
  printf("%d,", arr[2](4));
  printf("%d,", arr2[0](4));
  printf("%d,", arr2[1](4));
  printf("%d,", arr3[0](3));
  printf("%d,", callall(arr2, 4));
  printf("%d,", callarr(arr, 4));
  printf("%d,", arr[0] == dbl);
  printf("%d", arr[1] != dbl);
  return 0;
}
