public class Constant
{
    public static int FirstElement(int[] arr)
    {
        return arr[0];
    }

    public static void Main()
    {
        int[] data = { 10, 20, 30 };
        System.Console.WriteLine(FirstElement(data));
    }
}
