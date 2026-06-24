using NucleusBlazor;

var builder = WebApplication.CreateBuilder(args);

builder.Services.AddRazorComponents().AddInteractiveServerComponents();
// One embedded engine shared across all circuits.
builder.Services.AddSingleton<NucleusService>();

var app = builder.Build();

app.UseAntiforgery();
app.MapRazorComponents<App>().AddInteractiveServerRenderMode();

app.Run();
