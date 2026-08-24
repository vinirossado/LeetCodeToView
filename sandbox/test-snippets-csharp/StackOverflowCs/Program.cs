long Recurse(long n)
{
    long m = n + 1;
    return 1 + Recurse(m);
}

Console.WriteLine(Recurse(0));
