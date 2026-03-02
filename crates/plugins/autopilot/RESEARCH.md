Fortschrittliche Regelungsstrategien und Algorithmen für maritime Autopilotsysteme: Eine umfassende Analyse von der Kursstabilisierung bis zur computergestützten Segeloptimierung

Die Steuerung von Schiffen und insbesondere Segelyachten hat sich in den letzten Jahrzehnten von einer rein mechanischen Aufgabe zu einer hochkomplexen Disziplin der computergestützten Regelungstechnik entwickelt. Während frühe Autopiloten lediglich darauf ausgelegt waren, einen konstanten Kompasskurs zu halten, integrieren moderne Systeme eine Vielzahl von Parametern wie die Schiffsneigung, Segelpolardaten, Wellenmuster und die Dynamik hydraulischer Aktuatoren, um eine optimale Performance und Sicherheit zu gewährleisten. Die technologische Evolution spiegelt dabei den Übergang von einfachen Feedback-Schleifen zu prädiktiven und adaptiven Algorithmen wider, die in der Lage sind, komplexe nichtlineare Umwelteinflüsse in Echtzeit zu kompensieren.  
Mathematische Modellierung der Schiffsdynamik als Grundlage der Regelung

Jeder Entwurf eines Autopilotsystems beginnt mit der mathematischen Beschreibung der Schiffsbewegung. In der maritimen Forschung ist das nach Minorsky benannte Nomoto-Modell der Standard für die Modellierung der Gierdynamik eines Schiffes. Dieses Modell abstrahiert die komplexen hydrodynamischen Interaktionen auf eine handhabbare Anzahl von Parametern, die das Ansprechverhalten des Rumpfes auf Ruderbefehle beschreiben.  
Das Nomoto-Modell erster und zweiter Ordnung

Das Nomoto-Modell erster Ordnung ist aufgrund seiner Einfachheit und Robustheit die bevorzugte Wahl für kommerzielle Autopiloten. Es beschreibt die Beziehung zwischen dem Ruderwinkel δ und der Gierrate r (der zeitlichen Änderung des Kurses ψ) durch eine Zeitkonstante T und einen Verstärkungsfaktor K :  
Tr˙+r=Kδ

In der Laplace-Domäne ergibt sich daraus die Übertragungsfunktion für den Kurs ψ:
G(s)=δ(s)ψ(s)​=s(Ts+1)K​

Für Anwendungen, die eine höhere Präzision erfordern – etwa bei Hochgeschwindigkeitsmanövern oder der Untersuchung von Überschwingphänomenen –, wird das Nomoto-Modell zweiter Ordnung herangezogen. Dieses Modell berücksichtigt die Kopplung zwischen der Quergeschwindigkeit (Sway) und der Gierbewegung, was zu einer Übertragungsfunktion mit einem Zählerpol führt:  
G(s)=s(1+T1​s)(1+T2​s)K(1+T3​s)​

Hierbei repräsentieren T1​ und T2​ die Zeitkonstanten der kombinierten Sway-Yaw-Dynamik, während T3​ den Einfluss der Querbeschleunigung auf die Gierrate modelliert. Ein wesentliches Problem bei der Verwendung des Modells zweiter Ordnung in adaptiven Filtern ist die numerische Instabilität (Ill-Conditioning), die auftritt, wenn sich ein Pol und eine Nullstelle nahezu auslöschen, was eine präzise Parameteridentifikation erschwert.  
Parameteridentifikation und Geschwindigkeitsabhängigkeit

Die Koeffizienten K und T sind keine statischen Werte; sie variieren drastisch mit der Schiffsgeschwindigkeit u und der Schiffslänge l. Die Skalierung erfolgt typischerweise nach den Formeln:  
K=K0​lu​,T=T0​ul​

Diese Abhängigkeit macht deutlich, dass ein fest eingestellter Regler bei niedrigen Geschwindigkeiten zu träge und bei hohen Geschwindigkeiten zu aggressiv reagieren würde. Daher nutzen moderne Systeme adaptive Ansätze oder Gain-Scheduling, bei dem die Reglerparameter basierend auf der gemessenen Geschwindigkeit über Grund (SOG) oder durch das Wasser (STW) aus einer Look-up-Tabelle angepasst werden.  
Modelltyp	Zustandsvariablen	Primärer Einsatzbereich	Vorteile
Nomoto 1. Ordnung	Kurs, Gierrate	Standard-Autopiloten, Kursstabilisierung	

Einfachheit, Robustheit gegen Parameterrauschen 
Nomoto 2. Ordnung	Kurs, Gierrate, Querdrift	Manöversimulation, Rennsport-Autopiloten	

Modellierung von Überschwingen und Sway-Kopplung 
State-Space (4 DOF)	Surge, Sway, Yaw, Roll	DP-Systeme, komplexe Schiffsdynamik	

Vollständige Erfassung der Kopplungseffekte 
 
Klassische und adaptive Regelungsstrategien

Der am weitesten verbreitete Algorithmus in der maritimen Industrie ist der Proportional-Integral-Derivative-Regler (PID). Trotz seiner einfachen Struktur bietet er bei korrekter Parametrierung eine zuverlässige Kursführung.  
Die Komponenten des PID-Reglers im maritimen Kontext

Die Implementierung eines PID-Reglers für einen Autopiloten verarbeitet die Kursabweichung (Error) e=ψref​−ψact​.  

    Der Proportionalanteil (P): Bestimmt die unmittelbare Reaktion auf eine Kursabweichung. Ein zu hoher P-Wert führt zu Oszillationen, während ein zu niedriger Wert dazu führt, dass das Schiff den Sollkurs nur sehr langsam erreicht.  

    Der Integralanteil (I): Akkumuliert die verbleibende Abweichung über die Zeit. Dies ist entscheidend, um den Einfluss von konstantem Winddruck oder Strömungen (Leeway) zu eliminieren, die das Schiff permanent von seinem Kurs wegdrücken würden.  

    Der Derivativanteil (D): Reagiert auf die Änderungsrate des Fehlers (die Gierrate). Er wirkt dämpfend und verhindert, dass das Schiff beim Zurückkehren auf den Kurs über das Ziel hinausschießt (Overshoot).  

Ein kritischer Aspekt bei der digitalen Umsetzung ist das sogenannte Anti-Windup-Schema. Wenn das Ruder seinen mechanischen Anschlag erreicht, der Regler aber weiterhin einen Integralfehler aufbaut, käme es beim Zurückdrehen zu massiven Verzögerungen. Moderne Algorithmen begrenzen den Integralspeicher daher auf das mechanisch Mögliche.  
Modellprädiktive Regelung (MPC)

In den letzten Jahren hat die Modellprädiktive Regelung (MPC) als leistungsfähigere Alternative zum PID-Regler an Bedeutung gewonnen. MPC nutzt das mathematische Modell des Schiffes, um die zukünftige Bewegung über einen definierten Vorhersagehorizont zu berechnen.  

Der Algorithmus löst in jedem Zeitschritt ein Optimierungsproblem, um die Stellgröße (Ruderwinkel) so zu wählen, dass eine Kostenfunktion minimiert wird. Diese Kostenfunktion gewichtet typischerweise die Kursabweichung gegen den Energieverbrauch der Ruderanlage. Ein signifikanter Vorteil von MPC ist die Fähigkeit, Randbedingungen (Constraints) direkt in den Algorithmus zu integrieren, wie etwa die maximale Winkelgeschwindigkeit des Ruders oder Grenzwerte für die Krängung.  

Studien zeigen, dass MPC-basierte Systeme Kursabweichungen schneller korrigieren und gleichzeitig weniger Ruderbewegungen erfordern als klassische PID-Regler, insbesondere unter dem Einfluss von Wellenstörungen. Der Nachteil liegt in der hohen benötigten Rechenleistung, die leistungsstarke CPUs wie im B&G H5000 oder NKE HR Processor erfordert.  
Zustandsfilterung und Sensorfusion mittels Kalman-Filtern

Die Qualität der Regelung hängt unmittelbar von der Genauigkeit der Sensordaten ab. Auf einer schwankenden Segelyacht sind Kompass- und Winddaten jedoch mit massivem Rauschen und Bewegungsartefakten behaftet. Der Kalman-Filter ist hier das mathematische Werkzeug der Wahl, um aus verrauschten Messungen eine optimale Schätzung des tatsächlichen Zustands zu generieren.  
Der Vorhersage-Korrektur-Zyklus

Der Kalman-Filter arbeitet rekursiv in zwei Phasen :  

    Prädiktion: Das System nutzt das physikalische Modell (z.B. Nomoto), um den nächsten Zustand (Position, Kurs, Geschwindigkeit) vorherzusagen. Gleichzeitig wird die Unsicherheit (Kovarianz) dieser Vorhersage berechnet.

    Korrektur: Sobald eine neue Messung (z.B. vom GPS oder Kompass) eintrifft, wird die Vorhersage mit dem Messwert verglichen. Das Ergebnis ist ein gewichteter Mittelwert, wobei das Gewicht (Kalman-Gain) davon abhängt, ob das Modell oder der Sensor in diesem Moment als zuverlässiger eingestuft wird.  

Für maritime Anwendungen wird häufig der Extended Kalman Filter (EKF) eingesetzt, da die Transformation zwischen dem schiffseigenen Koordinatensystem und dem geografischen Nord-System (NED - North East Down) trigonometrische und somit nichtlineare Funktionen erfordert.  
Bias-Schätzung und IMU-Integration

Ein wesentliches Problem bei günstigen Gyroskopen ist die Drift (Bias). Ein fortschrittlicher Kalman-Filter kann diesen Bias als zusätzlichen Zustand in sein Modell aufnehmen und kontinuierlich schätzen. In Systemen wie dem Pypilot wird eine 9-Achsen-Inertial-Measurement-Unit (IMU) verwendet, die Beschleunigungs-, Drehraten- und Magnetfelddaten fusioniert. Die Herausforderung besteht darin, dass Beschleunigungssensoren auf einem Schiff nicht nur die Schwerkraft (für die Berechnung von Krängung und Pitch), sondern auch die Zentrifugalkräfte in Kurven und die vertikalen Beschleunigungen durch Wellen messen. Nur durch die Fusion mit Gyroskopdaten lässt sich die tatsächliche Orientierung des Schiffes stabil bestimmen.  
Wellenfilterung und Seegangskompensation

Eines der anspruchsvollsten Ziele der Autopiloten-Regelung ist es, nicht auf jede einzelne Welle zu reagieren. Wellen verursachen hochfrequente Oszillationen des Schiffes, die durch das Ruder nicht sinnvoll korrigiert werden können. Ein Regler, der versucht, diesen Bewegungen zu folgen, würde lediglich die mechanische Abnutzung und den Stromverbrauch massiv erhöhen, ohne den mittleren Kurs zu verbessern.  
Modellbasierte Wellenfilter

Moderne High-End-Systeme verwenden modellbasierte Wellenfilter. Hierbei wird das Schiffsbewegungsmodell um ein stochastisches Modell des Seegangs erweitert. Der Kalman-Filter trennt die Bewegung in zwei Komponenten:  

    Low-Frequency (LF) Motion: Die tatsächliche Kursabweichung durch Wind, Strömung und Ruderwirkung.

    High-Frequency (WF) Motion: Die rein welleninduzierte Oszillation.

Der Autopilot reagiert ausschließlich auf die LF-Komponente. Dies reduziert die Ruderaktivität und damit den Energiebedarf um bis zu 40 %, wie Messungen an Systemen wie dem NKE HR Prozessor belegen.  
Recovery Mode und Gust Response

Ein besonderes Merkmal von Systemen wie dem B&G H5000 Pilot ist der "Recovery Mode". Dieser Algorithmus erkennt, wenn das Schiff durch eine außergewöhnlich große Welle oder das Kielwasser eines anderen Schiffes massiv vom Kurs abgelenkt wird. In einem solchen Fall schaltet der Regler kurzzeitig auf eine aggressivere Charakteristik um, um das Schiff schnellstmöglich wieder auf den Sollkurs zu bringen, bevor er in den normalen, energieeffizienten Modus zurückkehrt.  

Die "Gust Response" hingegen nutzt Winddaten, um präventiv zu reagieren. Wenn eine Böe (Gust) gemeldet wird, bevor das Schiff Zeit hat zu krängen oder den Kurs zu ändern, kann der Autopilot bereits eine kleine Ruderkorrektur vornehmen, um die Luvgierigkeit abzufangen.  
Performance-Optimierung durch Segelpolardaten

Für Regatta-Segler ist nicht nur der Kurs, sondern die maximale Geschwindigkeit zum Ziel (Velocity Made Good - VMG) entscheidend. Moderne Autopiloten sind in der Lage, direkt auf Basis von Polardiagrammen zu steuern.  
Integration der Polar-VPP

Ein Polardiagramm (oft Ergebnis eines Velocity Prediction Program - VPP) beschreibt die Zielgeschwindigkeit des Schiffes für jede Kombination aus wahrer Windgeschwindigkeit (TWS) und wahrem Windwinkel (TWA). Diese Daten werden typischerweise als .pol, .csv oder .txt Dateien in das System geladen.  
Parameter	Bedeutung für den Algorithmus
TWS (True Wind Speed)	

Bestimmt die Skalierung der Regler-Aggressivität 
TWA (True Wind Angle)	

Referenzwert für das Erreichen des optimalen VMG 
BSP (Boat Speed)	

Feedback-Variable zur Überprüfung der Polar-Effizienz 
Heel (Krängung)	

Korrekturfaktor für die Luvgierigkeit und Ruder-Offset 
 
VMG-Optimierung und Windsteuerung

Der Autopilot kann so programmiert werden, dass er den Windwinkel hält, der laut Polardiagramm das beste VMG liefert. Dies ist besonders beim Kreuzen gegen den Wind oder beim Segeln vor dem Wind entscheidend. Der Regler vergleicht permanent den aktuellen TWA mit dem Ziel-TWA der Polartabelle. Weicht das Schiff ab, korrigiert der Pilot sanft den Kurs.  

Ein kritischer Punkt ist hierbei die Unterscheidung zwischen dem Steuern nach scheinbarem Wind (AWA) und wahrem Wind (TWA). Da sich der scheinbare Wind bei jeder Geschwindigkeitsänderung des Schiffes verschiebt, würde eine reine AWA-Steuerung zu einem instabilen Kurs führen, wenn das Schiff in einer Welle beschleunigt. Hochwertige Systeme wie der NKE HR Pilot oder B&G Hercules berechnen in Echtzeit den wahren Wind unter Berücksichtigung der Mastbewegung (3D Motion Correction), um eine ruhige und präzise Windsteuerung zu ermöglichen.  
Berücksichtigung der Schiffsneigung und Luvgierigkeit

Die Krängung (Heel) hat einen signifikanten Einfluss auf die Steuereigenschaften einer Segelyacht. Wenn ein Schiff krängt, verschiebt sich der Segeldruckpunkt nach Lee, während der Rumpfwiderstand asymmetrisch wird. Dies führt zur sogenannten Luvgierigkeit (Weather Helm).  
Der H-Faktor zur Krängungskompensation

Einfache Autopiloten bemerken die Luvgierigkeit erst, wenn das Schiff bereits vom Kurs abgekommen ist. Fortschrittliche Algorithmen, wie sie beispielsweise im CysBOX-System beschrieben werden, nutzen den Krängungswinkel als Feedforward-Variable. Die Reglergleichung wird um einen Term erweitert:  
Ruderwinkel=⋯+H⋅Kra¨ngungswinkel

Dieser H-Parameter kompensiert proaktiv die Tendenz des Schiffes, bei einer Böe anzuluven, noch bevor eine Kursabweichung messbar ist. Dies führt zu einer deutlich ruhigeren Kursführung und reduziert die notwendigen Ruderausschläge, da der Regler der physikalischen Ursache entgegenwirkt, anstatt nur den Fehler zu heilen.  
Aerodynamik des Ruders und Downwash

Ein interessanter Aspekt der hochpräzisen Regelung ist die Berücksichtigung des "Downwash" vom Kiel. Wenn das Schiff unter Krängung segelt, erzeugt der Kiel Auftrieb, was zu einer verwirbelten Abströmung führt, die das Ruder unter einem anderen Winkel erreicht als die Anströmung am Bug. In einem perfekt ausbalancierten System steht das Ruder nicht bei 0 Grad, sondern bei etwa 3 bis 5 Grad, um den minimalen Widerstand zu erzeugen. High-End-Autopiloten können diesen "Ruder-Offset" (Parameter O) automatisch lernen und als neuen Nullpunkt für die Oszillation verwenden.  
Automatisierung des Segeltrimms und hydraulische Systeme

Auf modernen Superyachten und zunehmend auch auf luxuriösen Fahrtenyachten wird der Autopilot mit der Steuerung der Segelwinden vernetzt. Dies ermöglicht eine umfassende Automatisierung, die über das reine Lenken hinausgeht.  
Assisted Sail Trim (AST) und Heel Limiter

Das von Harken und Jeanneau entwickelte AST-System integriert Sensordaten in die Windensteuerung. Eine zentrale Komponente ist der "Heel Limiter":  

    Der Nutzer definiert einen maximalen Krängungswinkel (z.B. 20 Grad).

    Überschreitet eine Böe diesen Wert, sendet der Controller einen Befehl an die hydraulische oder elektrische Großschotwinde, um die Schot kontrolliert zu fieren.  

    Sobald sich das Schiff wieder aufrichtet, wird die Schot automatisch wieder auf den ursprünglichen Trimmwert dichtgeholt.  

Dieses System nutzt Lastsensoren an den Winschen, um sicherzustellen, dass keine Überlastung auftritt, falls eine Schot klemmt oder ein Crewmitglied manuell eingreift. Technisch basiert dies auf einer synchronisierten Steuerung von Rewind-Winschen, die Schot sowohl fieren als auch dichtziehen können, ohne dass die Leine vom Gehäuse genommen werden muss.  
Regelung von hydraulischen Captive Reel Winches

Bei sehr großen Lasten kommen hydraulische Captive-Winden zum Einsatz. Die Regelung dieser Systeme ist aufgrund der Kompressibilität der Hydraulikflüssigkeit und der Nichtlinearität der Ventile eine Herausforderung. Moderne Ansätze verwenden hierfür oft eine Kombination aus PID-Regelung und Zustandsbeobachtern (Luenberger Observer), um die Last am Motor und die Seilgeschwindigkeit präzise zu schätzen.  

Besonders bei aktiver Heave-Kompensation (die Technologie, die Wellenbewegungen beim Ablassen von Lasten ausgleicht und nun auch für den Segeltrimm adaptiert wird) kommen variable Verdrängermotoren zum Einsatz. Die Regelung muss hierbei den Schwenkwinkel der Pumpe und des Motors simultan steuern, um auch bei niedrigen Geschwindigkeiten ein hohes Drehmoment und eine ruckfreie Bewegung zu gewährleisten.  
XTE-Regelung und Bahnführung

Neben dem Halten eines Winkels zum Wind oder Kompass ist die Navigation entlang einer vorgegebenen Route (Track-Keeping) eine Kernaufgabe. Hierbei ist der Cross Track Error (XTE) die primäre Regelgröße.  
Kaskadierte Regelung für Bahnführung

Die XTE-Regelung wird meist als kaskadiertes System implementiert:

    Äußerer Regelkreis (Bahngeschwindigkeit): Vergleicht die aktuelle GPS-Position mit der Ideallinie zwischen zwei Wegpunkten. Er berechnet einen notwendigen Kurs (Heading), um das Schiff zurück auf die Linie zu führen.

    Innerer Regelkreis (Kursregler): Ein schneller PID- oder MPC-Regler, der das Ruder so steuert, dass das vom äußeren Kreis vorgegebene Heading gehalten wird.  

Die Herausforderung besteht darin, dass Schiffe keine Schienenfahrzeuge sind. Wind und Strömung führen zu einer seitlichen Abdrift (Crab Angle). Ein intelligenter XTE-Algorithmus lernt diesen Abdriftwinkel über den Integralanteil des äußeren Reglers und steuert das Schiff schräg gegen die Versetzung, sodass die Bewegung über Grund (Course Over Ground - COG) exakt der geplanten Route entspricht.  
Bahnführung in der Landwirtschaft vs. Schifffahrt

Interessanterweise finden sich Parallelen zwischen maritimen XTE-Algorithmen und Systemen für autonome Traktoren. In beiden Feldern werden Pfad-Interpolationstechniken genutzt, um aus diskreten Wegpunkten eine glatte Kurve zu berechnen, die das Fahrzeug mit minimalem Ruderaufwand abfahren kann. In der Schifffahrt wird dies durch die Berücksichtigung des Wendekreises und der Manövriereigenschaften des jeweiligen Schiffstyps ergänzt, um das XTE bei Kursänderungen um bis zu 80 % zu reduzieren.  
Implementierung und Hardware-Ecosysteme

Die theoretischen Algorithmen müssen in robuster Hardware umgesetzt werden, die den extremen Bedingungen auf See standhält.
Kommerzielle High-End-Systeme

Hersteller wie B&G und NKE dominieren den Markt für professionelle Segel-Autopiloten. Ihre Systeme zeichnen sich durch spezialisierte Prozessoren aus, die weit über die Leistung einfacher NMEA-Konverter hinausgehen.
System	Prozessor / CPU	Besonderheiten	Zielgruppe
B&G H5000	Ultra-fast Performance CPU	

Web-Browser Interface, 3D Motion Correction, StartLine Features 
	Grand Prix Racing, Superyachts
NKE HR Processor	High Frequency Processor	

25Hz Datenrate, adaptive Wellenfilterung, Fokus auf Solo-Racing 
	Vendée Globe, Offshore Racing
Pypilot	Raspberry Pi / Orange Pi	

Open Source (GPLv3), hochgradig anpassbar, extrem kostengünstig 
	Fahrtensegler, DIY-Enthusiasten
 
Die Rolle von Open Source: Pypilot und SignalK

Ein bemerkenswerter Trend ist die Öffnung der Autopiloten-Technologie durch Open-Source-Projekte wie Pypilot. Diese Systeme ermöglichen es dem Nutzer, tief in die Algorithmen einzugreifen und eigene Filter zu implementieren. Pypilot nutzt dabei moderne Web-Technologien und den SignalK-Standard, um Daten zwischen verschiedenen Geräten auf dem Schiff (Plotter, Smartphone, Sensoren) auszutauschen.  

Ein technisches Highlight von Pypilot ist die Trennung von Kurscomputer und Motorcontroller. Während der Kurscomputer (z.B. ein Raspberry Pi Zero) die komplexen mathematischen Berechnungen und die Weboberfläche bereitstellt, übernimmt ein dedizierter Mikrocontroller (Arduino-basiert) die Echtzeit-Steuerung des Motors und die Überwachung von Strom und Spannung, um die Mosfets vor Überlastung zu schützen.  
Schlussfolgerungen und Ausblick

Moderne Autopilotsysteme für Schiffe haben den Status eines reinen mechanischen Helfers weit hinter sich gelassen. Durch die Integration von Kalman-Filtern zur Zustandsschätzung, modellbasierten Wellenfiltern und der direkten Einbindung von Polardiagrammen sind sie in der Lage, Schiffe effizienter und sicherer zu steuern, als es ein menschlicher Rudergänger über längere Zeiträume könnte.  

Die wichtigsten Erkenntnisse für die Konzeption eines fortschrittlichen Systems sind:

    Modellbasierte Ansätze: Die Verwendung von Nomoto-Modellen ermöglicht eine präzise Anpassung an die jeweilige Schiffsdynamik, insbesondere wenn diese geschwindigkeitsabhängig skaliert werden.  

    Sensorfusion: Nur durch die Kombination von 9-Achsen-IMUs mit GPS und Winddaten lässt sich eine stabile Kurs- und Windsteuerung realisieren, die immun gegen Wellenartefakte ist.  

    Proaktive Korrektur: Die Berücksichtigung von Krängungswerten (Heel Compensation) und Windböen (Gust Response) erlaubt es dem System, Korrekturen einzuleiten, bevor ein Kursfehler entsteht.  

    Vernetzung: Die Integration von Segeltrimmsystemen (AST) und Autopilot schafft eine synergetische Lösung, die besonders für Einhandsegler oder kleine Crews einen massiven Sicherheitsgewinn darstellt.  

Zukünftige Entwicklungen werden verstärkt Künstliche Intelligenz und Machine Learning nutzen, um die Schiffsparameter während der Fahrt automatisch zu lernen (Self-Tuning), ohne dass eine manuelle Eingabe von PID-Werten oder Polartabellen erforderlich ist. Projekte wie MTCAS zeigen zudem den Weg hin zu vollautonomen Systemen, die nicht nur den Kurs halten, sondern auch aktiv Kollisionen unter Einhaltung der KVR-Regeln vermeiden. Die Basis hierfür bleibt jedoch die solide Beherrschung der klassischen Regelungstechnik und der präzisen mathematischen Modellierung der physikalischen Prozesse auf See.  
sailrace.com
NKE HR Processor - SailRACE
Wird in einem neuen Fenster geöffnet
bandg.com
H5000 Instruments & Displays | Cruising / Racing - B&G Sailing Electronics
Wird in einem neuen Fenster geöffnet
chicagomarineelectronics.com
B&G H5000 Introduction - Chicago Marine Electronics
Wird in einem neuen Fenster geöffnet
researchgate.net
(PDF) COMPARISON BETWEEN PROPORTIONAL INTEGRAL ...
Wird in einem neuen Fenster geöffnet
researchgate.net
Learning-Based Nonlinear Model Predictive Controller for Hydraulic Cylinder Control of Ship Steering System - ResearchGate
Wird in einem neuen Fenster geöffnet
rsisinternational.org
Overview of Use of PID, Fuzzy Logic, and Model Predictive Control in Autonomous Vehicle Systems - RSIS International
Wird in einem neuen Fenster geöffnet
yadda.icm.edu.pl
CONSISTENT DESIGN OF PID CONTROLLERS FOR AN AUTOPILOT
Wird in einem neuen Fenster geöffnet
scispace.com
Fundamental Properties of Linear Ship Steering Dynamic Models - SciSpace
Wird in einem neuen Fenster geöffnet
researchgate.net
Ships Steering Autopilot Design by Nomoto Model - ResearchGate
Wird in einem neuen Fenster geöffnet
scribd.com
Exercise 6 | PDF - Scribd
Wird in einem neuen Fenster geöffnet
simrad-yachting.com
Teaching your autopilot to steer | Simrad USA
Wird in einem neuen Fenster geöffnet
cybele-sailing.com
CysBOX Autopilot Algorithm Tuning - Cybele Sailing
Wird in einem neuen Fenster geöffnet
researchgate.net
An Unscented Kalman Filter based wave filtering algorithm for dynamic ship positioning
Wird in einem neuen Fenster geöffnet
researchgate.net
(PDF) Overview of Use of PID, Fuzzy Logic, and Model Predictive Control in Autonomous Vehicle Systems - ResearchGate
Wird in einem neuen Fenster geöffnet
ijstr.org
Implementation Of Flight Control System Based On Kalman And PID Controller For UAV - ijstr
Wird in einem neuen Fenster geöffnet
dsp.stackexchange.com
Understanding an adaptative single neuron PID controller
Wird in einem neuen Fenster geöffnet
mdpi.com
Adaptive Single Neuron Anti-Windup PID Controller Based on the Extended Kalman Filter Algorithm - MDPI
Wird in einem neuen Fenster geöffnet
researchgate.net
(PDF) Comparative Analysis of NMPC and Fuzzy PID Controllers for Trajectory Tracking in Omni-Drive Robots: Design, Simulation, and Performance Evaluation - ResearchGate
Wird in einem neuen Fenster geöffnet
nke-marine-electronics.com
Processor X HR - nke Marine Electronics
Wird in einem neuen Fenster geöffnet
westmarine.com
B&G H5000 Hercules Central Processor Unit (CPU) - West Marine
Wird in einem neuen Fenster geöffnet
secwww.jhuapl.edu
Kalman Filters for Forecasting Open-Ocean White Shipping Location - JHU APL
Wird in einem neuen Fenster geöffnet
johnowhitaker.dev
TIL: Kalman Filters – johnowhitaker.dev
Wird in einem neuen Fenster geöffnet
shubham1925.github.io
Mini Project 1 | Kalman-Filter
Wird in einem neuen Fenster geöffnet
forum.openmarine.net
Use of IMU in pypilot - OpenMarine Forum
Wird in einem neuen Fenster geöffnet
phoenixketch.blogspot.com
Pypilot Open-Source Marine Autopilot for Hydraulic Steering - Phoenix's Flight
Wird in einem neuen Fenster geöffnet
openplotter.readthedocs.io
Pypilot - OpenPlotter 4.x.x - Read the Docs
Wird in einem neuen Fenster geöffnet
torarnj.folk.ntnu.no
PERFORMANCE OF SHIP NAVIGATION WAVE FILTERS WITH LOW ENCOUNTER FREQUENCY IN FOLLOWING SEAS - Tor Arne Johansen
Wird in einem neuen Fenster geöffnet
torarnj.folk.ntnu.no
Design of Inertial Navigation Systems for Marine Craft with Adaptive Wave Filtering aided by Triple-Redundant Sensor Packages
Wird in einem neuen Fenster geöffnet
nauticexpo.com
Boat NMEA data processing module - HR - nke Marine Electronics - NauticExpo
Wird in einem neuen Fenster geöffnet
bandg.com
H5000 Pilot Computer - B&G Sailing Electronics
Wird in einem neuen Fenster geöffnet
seapilot.com
Polars - Seapilot
Wird in einem neuen Fenster geöffnet
evolution-tactic.com
Polar Curves for Performance • EVOLUTION (en)
Wird in einem neuen Fenster geöffnet
raymarine.com
Polar Performance Data | Online Guides - Raymarine
Wird in einem neuen Fenster geöffnet
leopardcatamarans.com
Understanding Polar Diagrams: Sailing by The Number to Maximise Performance
Wird in einem neuen Fenster geöffnet
76trombones.wordpress.com
Polar diagrams – VMG | SO76TP "Routing for Non-Routers" - WordPress.com
Wird in einem neuen Fenster geöffnet
bandg.com
B&G H5000 Hercules Central Processor Unit (CPU) | B&G Sailing USA
Wird in einem neuen Fenster geöffnet
pysystems.com
B&G H5000 3D Motion Sensor - Pacific Yacht Systems
Wird in einem neuen Fenster geöffnet
onemetre.net
Weather helm (AMYA MY #182) - Lester Gilbert's
Wird in einem neuen Fenster geöffnet
quantumsails.com
Staying in Control in Breeze - Quantum Sails
Wird in einem neuen Fenster geöffnet
usvmyg.org
Helm Balance, with simple calculations of Center of Effort and Center of Lateral Resistance
Wird in einem neuen Fenster geöffnet
uk.boats.com
Assisted sail trim: push-button sailing - boats.com
Wird in einem neuen Fenster geöffnet
trends.nauticexpo.com
NEW: sail-trimming monitoring and control system by Harken
Wird in einem neuen Fenster geöffnet
yacht.de
Assisted Sail Trim: When the computer trims the sails | YACHT
Wird in einem neuen Fenster geöffnet
sailmagazine.com
Pittman Innovation: Jeanneau/Harken Assisted Sail Trim
Wird in einem neuen Fenster geöffnet
harken.com
Hydraulic & Electric Captive Reel Winches Q&A - Harken
Wird in einem neuen Fenster geöffnet
researchgate.net
Modeling, Simulation and Control of Hydraulic Winch System - ResearchGate
Wird in einem neuen Fenster geöffnet
researchgate.net
A Control Algorithm for Active/Passive Hydraulic Winches Used in Active Heave Compensation - ResearchGate
Wird in einem neuen Fenster geöffnet
harken.com
Hydraulic Captive Reel Winches - Harken
Wird in einem neuen Fenster geöffnet
researchgate.net
(PDF) Research on Three-Closed-Loop ADRC Position Compensation Strategy Based on Winch-Type Heave Compensation System with a Secondary Component - ResearchGate
Wird in einem neuen Fenster geöffnet
simrad-yachting.com
Understanding and using XTE | Simrad USA
Wird in einem neuen Fenster geöffnet
marinepublic.com
What is Cross-Track Error (XTE): Navigational Basics - Marine Public
Wird in einem neuen Fenster geöffnet
uknowledge.uky.edu
Methods for Calculating Relative Cross-Track Error for ASABE/ISO Standard 12188-2 from Discrete Measurements - UKnowledge
Wird in einem neuen Fenster geöffnet
mdpi.com
Assessment of the Steering Precision of a Hydrographic USV along Sounding Profiles Using a High-Precision GNSS RTK Receiver Supported Autopilot - MDPI
Wird in einem neuen Fenster geöffnet
opencpn-manuals.github.io
Pypilot Autopilot :: OpenCPN - GitHub Pages
Wird in einem neuen Fenster geöffnet
pypilot.org
pypilot_mfd
Wird in einem neuen Fenster geöffnet
pypilot.org
Hardware and Installation - PyPilot
Wird in einem neuen Fenster geöffnet
ptj.de
STATUSTAGUNG MARITIME TECHNOLOGIEN - Projektträger Jülich