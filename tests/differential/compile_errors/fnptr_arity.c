int g(int x) { return x; } int main() { int (*fp)(int) = g; return fp(1, 2); }
