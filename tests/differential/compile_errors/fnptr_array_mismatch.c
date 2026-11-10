int dbl(int x) { return x; }
int two(int a, int b) { return a + b; }
int main() {
  int (*arr[2])(int) = {dbl, two};
  return 0;
}
