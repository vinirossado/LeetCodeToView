var threads = new Thread[8];
for (int i = 0; i < threads.Length; i++)
{
    int id = i;
    threads[i] = new Thread(() => Console.WriteLine("thread " + id + " rodando"));
    threads[i].Start();
}
foreach (var t in threads)
{
    t.Join();
}
Console.WriteLine("todas as threads terminaram");
