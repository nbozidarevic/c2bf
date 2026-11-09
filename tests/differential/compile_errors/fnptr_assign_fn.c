int g(int x) { return x; } int main() { int (*fp)(int) = g; *fp = g; return 0; }
