#include <QCoreApplication>
#include "interactionsource.h"
#include "tracker.h"

#include <QGuiApplication>

int main(int argc, char *argv[])
{
    qputenv("QT_MESSAGE_PATTERN", "[%{type}:%{function}:%{line}] %{message}");
    //QCoreApplication a(argc, argv);
    QGuiApplication a(argc, argv);
    Tracker tracker(&InteractionSource::instance());

    return a.exec();
}
