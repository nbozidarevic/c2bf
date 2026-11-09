int main() {
  int x = 1;
  switch (x) {
    case 1:
      goto out;
  }
  out: printf("x");
}
