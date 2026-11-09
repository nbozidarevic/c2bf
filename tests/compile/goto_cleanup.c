#include <stdio.h>
int main() {
  int n = 0;
  int rc = 0;
  scanf("%d", &n);
  if (n < 0) {
    rc = 1;
    goto cleanup;
  }
  if (n > 10) {
    rc = 2;
    goto cleanup;
  }
  printf("ok %d,", n);
  cleanup: printf("rc=%d", rc);
}
