public class LinearSpaceAllocation
{
    public static int[] Duplicate(int[] arr)
    {
        int[] copy = new int[arr.Length];
        for (int i = 0; i < arr.Length; i++)
        {
            copy[i] = arr[i];
        }
        return copy;
    }

    public static void Main()
    {
        int[] data = { 1, 2, 3 };
        System.Console.WriteLine(Duplicate(data).Length);
    }
}
