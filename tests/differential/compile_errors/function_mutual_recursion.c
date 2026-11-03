int f(int a) { return g(a); }
int g(int a) { return f(a); }
int main() { return f(1); }
