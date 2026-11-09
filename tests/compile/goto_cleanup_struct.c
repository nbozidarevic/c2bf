#include <stdio.h>
int main() {
  int n = 0;
  int rc = 0;
  scanf("%d", &n);
  if (n < 0) {
    rc = 1;
  } else if (n > 10) {
    rc = 2;
  } else {
    printf("ok %d,", n);
  }
  printf("rc=%d", rc);
}
