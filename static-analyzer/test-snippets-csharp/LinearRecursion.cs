public class LinearRecursion
{
    public static int Factorial(int n)
    {
        if (n <= 1)
        {
            return 1;
        }
        return n * Factorial(n - 1);
    }

    public static void Main()
    {
        System.Console.WriteLine(Factorial(5));
    }
}
