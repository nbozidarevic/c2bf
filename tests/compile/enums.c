#include <stdio.h>

enum Color { RED, GREEN = 5, BLUE };
enum { A = 'a', B = A + 1 };
enum State { IDLE, RUNNING };

int main() {
  enum Color c = BLUE;
  enum State s = RUNNING;
  int x = A;
  {
    int RED = 9;
    printf("%d\n", RED);
  }
  printf("%d,%d,%d,%d,%d,%d\n", RED, GREEN, BLUE, x, B, s);
  switch (c) {
    case GREEN:
      printf("G\n");
      break;
    case BLUE:
      printf("B\n");
      break;
    default:
      printf("?\n");
      break;
  }
  return 0;
}
