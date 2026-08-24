public class LogDivide
{
    public static int CountHalvings(int n)
    {
        int steps = 0;
        while (n > 1)
        {
            n = n / 2;
            steps++;
        }
        return steps;
    }

    public static void Main()
    {
        System.Console.WriteLine(CountHalvings(1024));
    }
}
