public class DeepRecursion {
    static long recurse(long n) {
        return 1 + recurse(n + 1);
    }

    public static void main(String[] args) {
        try {
            recurse(0);
        } catch (StackOverflowError e) {
            System.out.println("stack overflow capturado");
            throw e;
        }
    }
}
