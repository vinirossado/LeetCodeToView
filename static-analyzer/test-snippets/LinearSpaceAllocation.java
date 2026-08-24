public class LinearSpaceAllocation {
    public static int[] duplicate(int[] arr) {
        int[] copy = new int[arr.length];
        for (int i = 0; i < arr.length; i++) {
            copy[i] = arr[i];
        }
        return copy;
    }

    public static void main(String[] args) {
        int[] data = {1, 2, 3};
        System.out.println(duplicate(data).length);
    }
}
