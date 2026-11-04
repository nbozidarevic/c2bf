#include <stdio.h>

typedef int MyInt;
typedef MyInt Alias;
typedef unsigned int U;
typedef int *PInt;
typedef char Arr[4];

MyInt twice(MyInt);
MyInt twice(MyInt x) {
  return x + x;
}

int main() {
  MyInt a = 21;
  Alias c = 1;
  U b = 3;
  PInt p = &a;
  Arr s = "hi";
  scanf("%d", p);
  *p = twice(*p);
  printf("%d,%d,%s,%d\n", a + b, *p, s, c);
  return 0;
}
