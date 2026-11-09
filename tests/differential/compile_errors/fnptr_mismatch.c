int g(int x) { return x; } int main() { int (*fp)(int, int) = g; return 0; }
