#include <stdio.h>
#define N 5
#define TEN 10
#define B 3
#define A B
#define MSG "hi\n"
#undef UNUSED

int main() {
  printf("%d\n", N);
  printf("%d\n", TEN + N);
  printf("%d\n", A);
  printf(MSG);
#define N 6
  printf("%d\n", N);
  return 0;
}
