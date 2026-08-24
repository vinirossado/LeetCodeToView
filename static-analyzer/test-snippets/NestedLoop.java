public class NestedLoop {
    public static int countPairs(int[] arr) {
        int count = 0;
        for (int i = 0; i < arr.length; i++) {
            for (int j = 0; j < arr.length; j++) {
                if (arr[i] == arr[j]) {
                    count++;
                }
            }
        }
        return count;
    }

    public static void main(String[] args) {
        int[] data = {1, 2, 3};
        System.out.println(countPairs(data));
    }
}
