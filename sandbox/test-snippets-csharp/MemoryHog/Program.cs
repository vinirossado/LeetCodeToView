var chunks = new List<byte[]>();
while (true)
{
    chunks.Add(new byte[10_000_000]); // 10MB por iteração
    Console.WriteLine("alocado: " + (chunks.Count * 10) + "MB");
}
