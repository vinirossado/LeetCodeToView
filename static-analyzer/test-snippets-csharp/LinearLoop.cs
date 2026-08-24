public class LinearLoop
{
    public static int Sum(int[] arr)
    {
        int total = 0;
        for (int i = 0; i < arr.Length; i++)
        {
            total += arr[i];
        }
        return total;
    }

    public static void Main()
    {
        int[] data = { 1, 2, 3, 4, 5 };
        System.Console.WriteLine(Sum(data));
    }
}
