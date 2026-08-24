public class LogDivide {
    public static int countHalvings(int n) {
        int steps = 0;
        while (n > 1) {
            n = n / 2;
            steps++;
        }
        return steps;
    }

    public static void main(String[] args) {
        System.out.println(countHalvings(1024));
    }
}
