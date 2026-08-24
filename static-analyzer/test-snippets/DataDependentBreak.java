public class DataDependentBreak {
    public static int findIndex(int[] arr, int target) {
        int result = -1;
        for (int i = 0; i < arr.length; i++) {
            if (arr[i] == target) {
                result = i;
                break;
            }
        }
        return result;
    }

    public static void main(String[] args) {
        int[] data = {5, 3, 8, 1, 9};
        System.out.println(findIndex(data, 8));
    }
}
