// Top-level statements with a local function (C#'s nested-function recursion idiom)
// — exercises local_function_statement handling, distinct from the plain
// global_statement path covered by TopLevelLoop.cs.
int Factorial(int n)
{
    if (n <= 1)
    {
        return 1;
    }
    return n * Factorial(n - 1);
}

System.Console.WriteLine(Factorial(5));
