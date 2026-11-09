int g(int x) { return x; } int main() { int (*fp)(int) = g; fp++; return 0; }
