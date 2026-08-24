public class CountLoop {
    public static void main(String[] args) {
        long sum = 0;
        for (int i = 0; i < 1000; i++) {
            sum += i;
        }
        System.out.println("soma: " + sum);
    }
}
