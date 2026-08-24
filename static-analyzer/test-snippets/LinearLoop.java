public class LinearLoop {
    public static int sum(int[] arr) {
        int total = 0;
        for (int i = 0; i < arr.length; i++) {
            total += arr[i];
        }
        return total;
    }

    public static void main(String[] args) {
        int[] data = {1, 2, 3, 4, 5};
        System.out.println(sum(data));
    }
}
