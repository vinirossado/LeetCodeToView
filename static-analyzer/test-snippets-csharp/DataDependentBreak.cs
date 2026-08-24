public class DataDependentBreak
{
    public static int FindIndex(int[] arr, int target)
    {
        int result = -1;
        for (int i = 0; i < arr.Length; i++)
        {
            if (arr[i] == target)
            {
                result = i;
                break;
            }
        }
        return result;
    }

    public static void Main()
    {
        int[] data = { 5, 3, 8, 1, 9 };
        System.Console.WriteLine(FindIndex(data, 8));
    }
}
