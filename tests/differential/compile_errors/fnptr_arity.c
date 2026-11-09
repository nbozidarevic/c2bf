int g(int x) { return x; } int main() { int (*fp)(int) = g; printf("%d", fp(1, 2)); return 0; }
