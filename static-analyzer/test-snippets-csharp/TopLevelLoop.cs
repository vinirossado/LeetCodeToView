// No enclosing class/method — a real, valid C# input shape (top-level statements),
// same as sandbox/test-snippets-csharp/Loop/Program.cs. Exercises the "top-level"
// synthetic method built by csharp_adapter.rs from `global_statement` nodes.
int x = 10;
long sum = 0;
for (int i = 0; i < x; i++)
{
    sum += i;
}
System.Console.WriteLine(sum);
