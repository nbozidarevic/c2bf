#include <stdio.h>

int dbl(int v);
void greet(void);
int add(int, int);
int peek(char *);
int first(char []);

char g[] = "G!";

int dbl(int x) { return x * 2; }
int add(int a, int b) { return a + b; }
int add(int, int);
void greet(void) { printf("hi "); }
int peek(char *p) { return *p; }
int first(char s[]) { return s[0]; }

int main() {
  printf("%d,", dbl(21));
  greet();
  printf("%d\n", add(3, 4));
  printf("%d,%d\n", peek(g), first(g));
  return 0;
}
