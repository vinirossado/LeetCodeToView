public class NestedLoop
{
    public static int CountPairs(int[] arr)
    {
        int count = 0;
        for (int i = 0; i < arr.Length; i++)
        {
            for (int j = 0; j < arr.Length; j++)
            {
                if (arr[i] == arr[j])
                {
                    count++;
                }
            }
        }
        return count;
    }

    public static void Main()
    {
        int[] data = { 1, 2, 3 };
        System.Console.WriteLine(CountPairs(data));
    }
}
